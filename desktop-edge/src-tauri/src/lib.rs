//! Ralleh desktop edge — Tauri command surface (Phase 1).
//!
//! Keep IPC allowlisted and narrow (threat model T11). No raw FS/net
//! exposure to the webview — settings I/O stays in Rust. OS capabilities
//! go through policy + traits (T13), never raw clipboard/mic APIs from JS.

mod assistant;
mod assistant_health;
mod audit;
mod diagnostics;
mod mic;
mod os_caps;
mod presence;
mod presence_health;
mod presence_log;
mod presence_mic;
mod presence_speaking;
mod secret_store;
mod settings;

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use assistant::{build_backend_from_config, completion_request, AssistantState, ECHO_CAPABILITY};
use audit::{record as audit_record, AuditEvent, AuditKind, AuditLog, SecretKind};
use mic::{mic_feature_enabled, run_mic_smoke, MicSmokeResult};
use os_caps::{run_clipboard_smoke, ClipboardSmokeResult};
use presence::{EventListener, Presence};
use presence_ipc::{
    Command as PresenceCommand, Event as PresenceEvent, PaletteId, PresenceMode, QualityTier,
};
use presence_mic::MicPump;
use ralleh_ai_router::{CompletionOutcome, CompletionResponse, CompletionStreamEvent};
#[cfg(feature = "playback")]
use ralleh_audio_core::CpalPlaybackSink;
use ralleh_audio_core::{run_mock_voice_pipeline, MockTts, MockVoicePipelineResult, TextToSpeech};
use ralleh_tool_gateway::ToolCallOutcome;
use secret_store::open_default as open_default_secret_store;
use settings::PresencePosition;
use settings::{
    load_settings, migrate_completion_secret, migrate_settings_file, save_settings,
    settings_path_display, write_completion_config, ApiKeyUpdate, CompletionConfigUpdate,
    EdgeSettings, RedactedCompletionConfig, SettingsMigrationOutcome,
};
use tauri::ipc::Channel;

const EDGE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeSettingsResponse {
    #[serde(flatten)]
    pub settings: EdgeSettings,
    /// Derived gate: critical fields present (not a stored flag).
    pub setup_complete: bool,
}

impl EdgeSettingsResponse {
    fn from_settings(settings: EdgeSettings) -> Self {
        let setup_complete = settings.is_complete();
        Self {
            settings,
            setup_complete,
        }
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EdgeFeatures {
    pub mic: bool,
    pub clipboard_os: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreStatus {
    pub product: String,
    pub edge: String,
    pub version: String,
    pub message: String,
    pub features: EdgeFeatures,
}

#[tauri::command]
fn core_ping() -> CoreStatus {
    CoreStatus {
        product: "Ralleh".into(),
        edge: "desktop".into(),
        version: EDGE_VERSION.into(),
        message: "Rust edge core is reachable.".into(),
        features: EdgeFeatures {
            mic: mic_feature_enabled(),
            clipboard_os: cfg!(feature = "clipboard-os"),
        },
    }
}

/// Fires the visual `Error` pulse when a smoke handler failed. Every
/// user-triggered capability check maps `Err(_)` to either a policy
/// denial (T13) or a handler failure (mic device gone, clipboard
/// backend refused, etc.) — both are exactly the "map real outcomes
/// → modes" cases Phase 3 §3.5 wants surfaced visually. Runs on the
/// Tauri command thread; the pulse itself is async on the presence
/// side (see `Presence::pulse_error`).
fn pulse_on_err<T>(result: &Result<T, String>, presence: &Presence) {
    if result.is_err() {
        presence.pulse_error();
    }
}

/// Run the mock voice pipeline and drive the speaking-mode visual +
/// audible playback. Two arities exist: with `playback` enabled the
/// synthesized PCM is enqueued to `CpalPlaybackSink` so the operator
/// hears the response; without the feature, playback is silent (still
/// a visible pulse). Split into two `#[tauri::command]` functions
/// because Tauri's command macro doesn't handle `#[cfg]` on
/// arguments -- the shared body lives in `run_voice_smoke`.
#[cfg(feature = "playback")]
#[tauri::command]
fn voice_smoke(
    presence: State<'_, Presence>,
    playback: State<'_, PlaybackSinkState>,
) -> Result<MockVoicePipelineResult, String> {
    run_voice_smoke(&presence, Some(&playback))
}

#[cfg(not(feature = "playback"))]
#[tauri::command]
fn voice_smoke(presence: State<'_, Presence>) -> Result<MockVoicePipelineResult, String> {
    run_voice_smoke(&presence)
}

fn run_voice_smoke(
    presence: &Presence,
    #[cfg(feature = "playback")] playback: Option<&PlaybackSinkState>,
) -> Result<MockVoicePipelineResult, String> {
    let result = run_mock_voice_pipeline();
    match &result {
        Ok(r) => {
            // Duration of the synthesized speech in wall-clock
            // terms. Sub-second utterances still get a full
            // "speaking" hold rather than a blink.
            let ms = ((r.tts_samples as u64) * 1_000)
                .checked_div(r.sample_rate_hz.max(1) as u64)
                .unwrap_or(0);
            // Re-synthesize the transcript so we own the PCM twice:
            // once for the RMS pump that drives the visual envelope
            // (§3.3), once for the cpal sink that actually makes the
            // TTS audible. `MockTts` is a synchronous constant-tone
            // generator so the double-synthesis is effectively free.
            // When real streaming TTS lands, both consumers move to
            // tapping a shared ringbuffer instead of holding copies.
            if let Ok(audio) = MockTts::new().synthesize(&r.transcript) {
                if let Some(tx) = presence.sender_clone() {
                    presence_speaking::spawn(audio.samples.clone(), audio.sample_rate_hz, tx);
                }
                #[cfg(feature = "playback")]
                if let Some(state) = playback {
                    if let Ok(guard) = state.lock() {
                        if let Some(sink) = guard.as_ref() {
                            sink.enqueue(&audio.samples, audio.sample_rate_hz);
                        }
                    }
                }
            }
            presence.pulse_speaking(ms);
        }
        Err(_) => presence.pulse_error(),
    }
    result
}

#[tauri::command]
fn clipboard_smoke(
    app: AppHandle,
    presence: State<'_, Presence>,
) -> Result<ClipboardSmokeResult, String> {
    let result = load_settings(&app).and_then(|s| run_clipboard_smoke(&s));
    pulse_on_err(&result, &presence);
    result
}

#[tauri::command]
fn mic_smoke(app: AppHandle, presence: State<'_, Presence>) -> Result<MicSmokeResult, String> {
    // ~1s is enough to prove device open + frames without freezing the UI long.
    let result = load_settings(&app).and_then(|s| run_mic_smoke(&s, 1.0));
    pulse_on_err(&result, &presence);
    result
}

#[tauri::command]
fn load_edge_settings(app: AppHandle) -> Result<EdgeSettingsResponse, String> {
    Ok(EdgeSettingsResponse::from_settings(load_settings(&app)?))
}

#[tauri::command]
fn save_edge_settings(
    app: AppHandle,
    settings: EdgeSettings,
) -> Result<EdgeSettingsResponse, String> {
    Ok(EdgeSettingsResponse::from_settings(save_settings(
        &app, &settings,
    )?))
}

#[tauri::command]
fn edge_settings_path(app: AppHandle) -> Result<String, String> {
    settings_path_display(&app)
}

// -----------------------------------------------------------------------------
// Presence commands (Phase 2 §3)
//
// Every command below is a thin translation from the JS-friendly argument
// shape Tauri hands us into a `presence_ipc::Command`, then a fire-and-forget
// send. The `Presence` state itself handles the case where the renderer is
// disabled (no `RALLEH_PRESENCE_BIN`) or the child has exited, so these
// commands never surface an error to the UI for a missing presence — that
// would be a startup-time misconfiguration, not a per-invocation failure.
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceStatus {
    pub enabled: bool,
    /// Milliseconds since the shell last observed *any* reverse-channel
    /// event from the runtime (heartbeat, `Ready`, or `Moved`). `None`
    /// when presence is disabled or no event has arrived yet. A value
    /// larger than `presence_ipc::STALL_THRESHOLD_MS` indicates the
    /// runtime is not currently emitting — the audit log will already
    /// contain a `PresenceStalled` event by that point.
    pub last_event_ms_ago: Option<u64>,
    /// Highest heartbeat sequence ever seen. `None` before the first
    /// heartbeat. Useful for spotting a runtime restart-inside-process.
    pub last_heartbeat_sequence: Option<u64>,
    /// Runtime `uptime_ms` reported on the last heartbeat. `None`
    /// before the first heartbeat.
    pub last_heartbeat_uptime_ms: Option<u64>,
}

#[tauri::command]
fn presence_status(presence: State<'_, Presence>) -> PresenceStatus {
    let snap = presence.liveness_snapshot();
    let last_event_ms_ago = snap
        .last_event_at
        .map(|t| std::time::Instant::now().duration_since(t).as_millis() as u64);
    PresenceStatus {
        enabled: presence.is_enabled(),
        last_event_ms_ago,
        last_heartbeat_sequence: snap.last_heartbeat_sequence,
        last_heartbeat_uptime_ms: snap.last_heartbeat_uptime_ms,
    }
}

#[tauri::command]
fn presence_set_mode(
    mode: PresenceMode,
    engaged: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetMode { mode, engaged });
    Ok(())
}

/// Continuous-signal packet. Matches `presence_ipc::Signals` verbatim
/// (serde-side rename_all = "camelCase" to match the pattern the other
/// Tauri commands use for JS args). Kept as a local struct rather than
/// reusing the ipc type directly so a future divergence — for example a
/// clamped `audio_level_smoothed` computed on the shell side — doesn't
/// require a wire-version bump.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSignalsArgs {
    pub intensity: f32,
    pub audio_level: f32,
    pub progress: f32,
    #[serde(default)]
    pub active_modes: Vec<PresenceMode>,
}

#[tauri::command]
fn presence_set_signals(
    signals: PresenceSignalsArgs,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetSignals(presence_ipc::Signals {
        intensity: signals.intensity,
        audio_level: signals.audio_level,
        progress: signals.progress,
        active_modes: signals.active_modes,
    }));
    Ok(())
}

#[tauri::command]
fn presence_set_reduced_motion(
    enabled: bool,
    app: AppHandle,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetReducedMotion { enabled });
    update_presence_settings(&app, |s| s.presence_reduced_motion = enabled);
    Ok(())
}

/// OS-preference companion to `presence_set_reduced_motion`
/// (Phase 4 kickoff). Applies reduced motion to the running
/// presence **without** persisting to `EdgeSettings`, so the OS
/// accessibility preference can drive the runtime for a session
/// without silently overwriting a user's explicit toggle on the
/// dev panel. The frontend hooks
/// `matchMedia('(prefers-reduced-motion: reduce)')` and calls
/// this on mount + on change.
///
/// Design note: this is a session-only override deliberately. If
/// the user explicitly toggles via the dev panel, that goes
/// through the persisting `presence_set_reduced_motion` command
/// and survives restart — the OS pref can layer over that on
/// next boot without corrupting the stored value.
#[tauri::command]
fn presence_apply_reduced_motion(
    enabled: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetReducedMotion { enabled });
    Ok(())
}

/// Read-only snapshot of the modes the shell currently believes are
/// engaged on the runtime (Phase 4 accessibility). Returns
/// `PresenceMode::label()` strings so the caller doesn't need the
/// enum bindings; the aria-live status line consumes this directly
/// on a ~5 Hz timer. Sorted by label for deterministic UI order —
/// see the tracker in `presence::Presence::current_modes`.
#[tauri::command]
fn presence_current_modes(presence: State<'_, Presence>) -> Vec<String> {
    presence
        .current_modes()
        .into_iter()
        .map(|m| m.label().to_string())
        .collect()
}

#[tauri::command]
fn presence_set_palette(
    palette: PaletteId,
    app: AppHandle,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetPalette { palette });
    update_presence_settings(&app, |s| s.presence_palette = Some(palette));
    Ok(())
}

#[tauri::command]
fn presence_set_ring_wanted(wanted: bool, presence: State<'_, Presence>) -> Result<(), String> {
    presence.send(PresenceCommand::SetRingWanted { wanted });
    Ok(())
}

#[tauri::command]
fn presence_set_quality_tier(
    tier: QualityTier,
    app: AppHandle,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.send(PresenceCommand::SetQualityTier { tier });
    update_presence_settings(&app, |s| s.presence_quality_tier = Some(tier));
    Ok(())
}

#[tauri::command]
fn presence_set_interactive(
    interactive: bool,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    // Click-through toggle. Only meaningful when the runtime is in
    // transparent mode (see `Presence::spawn` env var), but harmless
    // otherwise — the wire type accepts it either way and the runtime
    // just calls `set_cursor_hittest` on whatever window it has.
    presence.send(PresenceCommand::SetInteractive { interactive });
    Ok(())
}

/// Handle to the active mic pump (if any). Mutex because Tauri hands
/// out shared references to managed state and we need to swap the
/// `Option` in-place from the start/stop commands.
type MicPumpState = Mutex<Option<MicPump>>;

/// Tauri-managed handle to the shell's default speaker sink. `None`
/// on hosts where `try_open_default` soft-skipped (CI, headless dev)
/// or where the OS refused to open an output device -- callers must
/// tolerate `None` and simply omit audible playback rather than
/// erroring.
#[cfg(feature = "playback")]
type PlaybackSinkState = Mutex<Option<CpalPlaybackSink>>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceMicStatus {
    pub running: bool,
    pub mic_feature: bool,
}

#[tauri::command]
fn presence_mic_status(pump: State<'_, MicPumpState>) -> PresenceMicStatus {
    PresenceMicStatus {
        running: pump.lock().map(|p| p.is_some()).unwrap_or(false),
        mic_feature: mic_feature_enabled(),
    }
}

#[tauri::command]
fn presence_mic_start(
    app: AppHandle,
    presence: State<'_, Presence>,
    pump: State<'_, MicPumpState>,
) -> Result<PresenceMicStatus, String> {
    // Same clearance gate as `mic_smoke`. Live capture without an
    // explicit acknowledgement is exactly the case T13 (policy +
    // capabilities) rules out.
    let settings = load_settings(&app)?;
    if !settings.mic_acknowledged {
        return Err(
            "mic clearance not stamped — open the station log (Voice) and acknowledge OS mic guidance first"
                .into(),
        );
    }

    let mut guard = pump.lock().map_err(|e| e.to_string())?;
    if guard.is_some() {
        // Already running — idempotent success. Restarting would drop
        // the current stream and open a new one; the UI has no way to
        // know it did that from a click and it would be a spec change.
        return Ok(PresenceMicStatus {
            running: true,
            mic_feature: mic_feature_enabled(),
        });
    }
    let Some(sender) = presence.sender_clone() else {
        return Err(
            "presence renderer is not running (set RALLEH_PRESENCE_BIN before starting the shell)"
                .into(),
        );
    };
    let started = MicPump::start(sender)?;
    *guard = Some(started);
    Ok(PresenceMicStatus {
        running: true,
        mic_feature: mic_feature_enabled(),
    })
}

// -----------------------------------------------------------------------------
// Assistant commands (Phase 3 §3.2)
//
// `thinking` and `tool_use` come from real work sources — the router and the
// tool gateway — rather than dev-panel toggles. Both handlers hold a
// `Presence::ModeHold` for the duration of the async call so the mode
// engagement matches the wall-clock time of the operation exactly, and pulse
// `Error` on any Denied / ApprovalRequired / Failed outcome.
// -----------------------------------------------------------------------------

#[tauri::command]
async fn assistant_think(
    prompt: String,
    app: AppHandle,
    state: State<'_, AssistantState>,
    presence: State<'_, Presence>,
) -> Result<String, String> {
    let settings = load_settings(&app)?;
    let request = completion_request(
        &settings.tenant_id,
        &settings.device_id,
        &settings.actor_id,
        &prompt,
    );
    // `router` cloned out before the await so we do not hold the
    // Tauri `State` guard longer than the async call. The `_hold`
    // guard, on the other hand, deliberately lives to the end of
    // scope: its `Drop` fires the mode-release even on early return
    // via `?`, panic, or a task cancellation.
    let router = state.router.clone();
    // Two drop-scoped guards — order matters at drop only insofar as
    // both fire; a slot in `in_flight` is released ~immediately
    // after the mode-release on the wire, which is what the scan
    // sweep wants to see before it starts firing attention pulses.
    let _work = state.begin_work();
    let _hold = presence.hold_mode(PresenceMode::Thinking);

    match router.route(&request).await {
        CompletionOutcome::Succeeded(CompletionResponse { text, .. }) => Ok(text),
        CompletionOutcome::Denied => {
            presence.pulse_error();
            Err("policy denied the completion request".into())
        }
        CompletionOutcome::ApprovalRequired => {
            presence.pulse_error();
            Err("completion requires human approval".into())
        }
        CompletionOutcome::Failed { backend, error } => {
            presence.pulse_error();
            Err(format!("completion failed via {backend}: {error}"))
        }
        CompletionOutcome::NoBackendConfigured => {
            presence.pulse_error();
            Err("no completion backend is configured on this shell".into())
        }
    }
}

/// Streaming counterpart to [`assistant_think`]. Same policy, same
/// mode engagements (Thinking + WorkGuard) — differs only in that
/// the response is delivered incrementally through the caller-
/// supplied Tauri `Channel<CompletionStreamEvent>` instead of
/// returned as one string.
///
/// The channel receives every `Chunk` in order, followed by
/// exactly one terminal event (`Done` / `Failed` / `Denied` /
/// `ApprovalRequired` / `NoBackendConfigured`). The command
/// returns `Ok(())` once the terminal event has been dispatched
/// — callers should await both the return and the terminal event
/// on the channel (they will happen together).
#[tauri::command]
async fn assistant_think_stream(
    prompt: String,
    on_event: Channel<CompletionStreamEvent>,
    app: AppHandle,
    state: State<'_, AssistantState>,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    let settings = load_settings(&app)?;
    let request = completion_request(
        &settings.tenant_id,
        &settings.device_id,
        &settings.actor_id,
        &prompt,
    );
    let _work = state.begin_work();
    let _hold = presence.hold_mode(PresenceMode::Thinking);
    let mut rx = state.router.route_stream(&request);
    let mut errored = false;
    while let Some(event) = rx.recv().await {
        // Track whether we saw a non-success terminal so the
        // presence's error pulse fires exactly once at the end,
        // in the same shape `assistant_think` (non-streaming)
        // uses. Emitting the event is a best-effort — a dropped
        // frontend receiver means the operator navigated away and
        // there's nothing to signal.
        match &event {
            CompletionStreamEvent::Failed { .. }
            | CompletionStreamEvent::Denied
            | CompletionStreamEvent::ApprovalRequired
            | CompletionStreamEvent::NoBackendConfigured => {
                errored = true;
            }
            _ => {}
        }
        if on_event.send(event).is_err() {
            log::debug!("assistant_think_stream: receiver dropped mid-stream");
            break;
        }
    }
    if errored {
        presence.pulse_error();
    }
    Ok(())
}

#[tauri::command]
fn assistant_tool_ping(
    app: AppHandle,
    state: State<'_, AssistantState>,
    presence: State<'_, Presence>,
) -> Result<String, String> {
    let settings = load_settings(&app)?;
    let gateway = state.gateway.clone();
    // Synchronous dispatch — `ToolGateway::dispatch` is sync today,
    // so no need to spawn or await. Both guards drop at end of
    // scope, including on any early return via `?`.
    let _work = state.begin_work();
    let _hold = presence.hold_mode(PresenceMode::ToolUse);

    let event = gateway.dispatch(
        settings.tenant_id.clone(),
        settings.device_id.clone(),
        settings.actor_id.clone(),
        ECHO_CAPABILITY.to_string(),
        serde_json::json!({ "source": "assistant_tool_ping" }),
    );
    match event.outcome {
        ToolCallOutcome::Succeeded { result_summary } => Ok(result_summary),
        ToolCallOutcome::Denied
        | ToolCallOutcome::ApprovalRequired
        | ToolCallOutcome::ApprovalRejected
        | ToolCallOutcome::Failed { .. }
        | ToolCallOutcome::NoHandlerRegistered
        | ToolCallOutcome::UnknownCapability => {
            presence.pulse_error();
            Err(format!(
                "tool call ended in a non-success outcome: {:?}",
                event.outcome
            ))
        }
    }
}

/// Sparse "look here" pulse (§3.4). Fires `Attention` for a short
/// hold — used by the notification / inbound-stream surface and by
/// the dev panel's Notify chip. Held-hold defaults to 450 ms, which
/// sits inside the runtime's 300–900 ms transition window and reads
/// as one deliberate glance rather than a fidget.
#[tauri::command]
fn assistant_notify_inbound(
    duration_ms: Option<u64>,
    presence: State<'_, Presence>,
) -> Result<(), String> {
    presence.pulse_attention(duration_ms.unwrap_or(450));
    Ok(())
}

/// Read-side of the backend config surface. Returns the currently
/// active backend name (what the router will send the next request
/// through, whatever its origin -- settings, env, or Echo fallback)
/// plus the persisted UI-owned config, redacted so the api_key
/// never leaves the Rust side. `None` for `configured` means the
/// operator has never opened the settings UI; the shell is running
/// on env-var + Echo defaults.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendStatus {
    pub active_backend: String,
    pub configured: Option<RedactedCompletionConfig>,
    /// Latest health-probe snapshot, or `Unknown` before the
    /// first probe completes. Frontend uses this to render a
    /// reachability badge alongside the configured backend
    /// name.
    pub health: assistant_health::HealthSnapshot,
}

/// Build an [`AuditEvent`] pre-populated with the shell's current
/// identity triple. Loading settings on every audit emit adds a
/// couple of ms but keeps the tenant/device/actor fields honest —
/// if the operator changed identity mid-session the audit trail
/// reflects that immediately instead of a cached value.
pub(crate) fn audit_event_with_identity(app: &AppHandle, kind: AuditKind) -> AuditEvent {
    let (tenant, device, actor) = match load_settings(app) {
        Ok(s) => (s.tenant_id, s.device_id, s.actor_id),
        Err(_) => (String::new(), String::new(), String::new()),
    };
    AuditEvent::new(kind, &tenant).with_identity(tenant, device, actor)
}

/// Heuristic: does an error message look like an egress denial?
/// The three enforcement layers all format their errors with the
/// literal string `"egress policy"`, so this is stable — but kept
/// small so a rename in one place doesn't silently break the
/// audit signal.
fn looks_like_egress_denial(err: &str) -> bool {
    err.to_lowercase().contains("egress policy")
}

#[tauri::command]
fn assistant_backend_status(
    app: AppHandle,
    state: State<'_, AssistantState>,
    health: State<'_, assistant_health::Health>,
) -> Result<BackendStatus, String> {
    let settings = load_settings(&app)?;
    let store = open_default_secret_store();
    let health_snapshot = {
        let guard = health.lock().map_err(|e| e.to_string())?;
        guard.materialize(std::time::Instant::now())
    };
    Ok(BackendStatus {
        active_backend: state.current_backend_name(),
        configured: settings
            .completion
            .as_ref()
            .map(|c| RedactedCompletionConfig::from_config_and_store(c, store.as_ref())),
        health: health_snapshot,
    })
}

/// Force an immediate health probe against the router's active
/// backend. Useful for the settings UI's "check now" button so
/// operators don't wait a full interval to see the impact of a
/// backend change. Ignores the disabled kill switch — the point
/// is that a human just asked.
#[tauri::command]
async fn assistant_probe_backend(
    app: AppHandle,
) -> Result<assistant_health::HealthSnapshot, String> {
    // The blocking `probe_once` needs an `AssistantState`
    // reference; grab a fresh one and release the state guard
    // before we do any awaits so a slow probe doesn't pin the
    // managed-state map.
    let (backend_name, outcome) = {
        // Scope the state guard so it's released before we lock
        // the health mutex below. A `tauri::State` doesn't need
        // an explicit drop -- it isn't `Drop` -- but scoping is
        // clearer than a `let _ = state;` pin.
        let state: State<'_, AssistantState> = app.state();
        assistant_health::probe_once(state.inner())
    };
    let health: State<'_, assistant_health::Health> = app.state();
    let now = std::time::Instant::now();
    let edge = {
        let mut guard = health.lock().map_err(|e| e.to_string())?;
        assistant_health::fold_outcome(&mut guard, backend_name.clone(), outcome.clone(), now)
    };
    // Mirror the automatic probe's audit behaviour on manual
    // probes so an operator kicking off a check leaves the same
    // paper trail a scheduled check would.
    if let Some(audit) = app.try_state::<AuditLog>() {
        match &edge {
            assistant_health::HealthEdge::BecameHealthy => {
                let latency = match &outcome {
                    assistant_health::ProbeOutcome::Ok { latency_ms } => Some(*latency_ms),
                    _ => None,
                };
                let _ = audit.write(
                    &audit_event_with_identity(&app, AuditKind::RouterHealthy)
                        .with_subject(format!("router:{backend_name}"))
                        .with_detail(serde_json::json!({
                            "latency_ms": latency,
                            "trigger": "manual",
                        })),
                );
            }
            assistant_health::HealthEdge::BecameUnhealthy => {
                let (latency, error) = match &outcome {
                    assistant_health::ProbeOutcome::Failed { latency_ms, error } => {
                        (Some(*latency_ms), Some(error.clone()))
                    }
                    _ => (None, None),
                };
                let _ = audit.write(
                    &audit_event_with_identity(&app, AuditKind::RouterUnhealthy)
                        .with_subject(format!("router:{backend_name}"))
                        .with_detail(serde_json::json!({
                            "latency_ms": latency,
                            "error": error,
                            "trigger": "manual",
                        })),
                );
            }
            assistant_health::HealthEdge::None => {}
        }
    }
    let health_guard = health.lock().map_err(|e| e.to_string())?;
    Ok(health_guard.materialize(std::time::Instant::now()))
}

/// Result the `assistant_test_backend` command returns. `ok` is
/// the top-level pass/fail because the UI needs to switch between
/// "green check" and "red alert" without pattern-matching on the
/// message. `latency_ms` is present on success so the UI can show
/// "Anthropic responded in 320 ms", which is a real signal when
/// choosing between models.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendTestResult {
    pub ok: bool,
    pub backend: String,
    pub latency_ms: Option<u64>,
    pub sample_response: Option<String>,
    pub error: Option<String>,
}

/// Try the proposed config against a real completion. Uses a
/// throwaway `AiRouter` so a failing test cannot corrupt the
/// production router state, and passes a short benign prompt so
/// providers that meter per-token don't charge for the smoke.
#[tauri::command]
async fn assistant_test_backend(
    app: AppHandle,
    audit_log: State<'_, AuditLog>,
    config: CompletionConfigUpdate,
) -> Result<BackendTestResult, String> {
    // Fold the update against whatever secret is currently stored
    // so `api_key: Keep` resolves to the real key -- otherwise
    // "test" would fail whenever the user edits a model without
    // re-entering the key. The store is the authoritative source;
    // any leftover cleartext key on disk is a strict pre-migration
    // fallback.
    let store = open_default_secret_store();
    let disk = load_settings(&app).ok().and_then(|s| s.completion);
    let existing_secret = store.read(config.kind).ok().flatten().or_else(|| {
        disk.as_ref()
            .and_then(|c| c.api_key.clone())
            .filter(|s| !s.is_empty())
    });
    let kind_label = config.kind.label().to_string();
    let requested_kind = config.kind;
    let requested_url = config.base_url.clone();
    let cfg = match config.into_config_with_secret(existing_secret) {
        Ok(c) => c,
        Err(reason) => {
            // Surface validation errors as a "test failed" result
            // rather than a Tauri command error so the UI can render
            // them in the same status area as network failures.
            // Egress denials produce their own audit line so a
            // security review can spot the "user tried to point us
            // at attacker.example" attempt without spelunking logs.
            if looks_like_egress_denial(&reason) {
                audit_record(
                    &audit_log,
                    audit_event_with_identity(&app, AuditKind::EgressDeny)
                        .with_subject(format!("{}@{}", requested_kind.label(), requested_url))
                        .with_detail(serde_json::json!({
                            "stage": "test",
                            "reason": reason,
                        })),
                );
            }
            return Ok(BackendTestResult {
                ok: false,
                backend: kind_label,
                latency_ms: None,
                sample_response: None,
                error: Some(reason),
            });
        }
    };

    let backend = match build_backend_from_config(&cfg) {
        Ok(b) => b,
        Err(reason) => {
            if looks_like_egress_denial(&reason) {
                audit_record(
                    &audit_log,
                    audit_event_with_identity(&app, AuditKind::EgressDeny)
                        .with_subject(format!("{}@{}", cfg.kind.label(), cfg.base_url))
                        .with_detail(serde_json::json!({
                            "stage": "test-build",
                            "reason": reason,
                        })),
                );
            }
            return Ok(BackendTestResult {
                ok: false,
                backend: cfg.kind.label().to_string(),
                latency_ms: None,
                sample_response: None,
                error: Some(reason),
            });
        }
    };
    // URL passed every enforcement layer -- record the allow so a
    // review can prove "no covert routing" instead of just
    // "no visible failures".
    audit_record(
        &audit_log,
        audit_event_with_identity(&app, AuditKind::EgressAllow)
            .with_subject(format!("{}@{}", cfg.kind.label(), cfg.base_url))
            .with_detail(serde_json::json!({ "stage": "test" })),
    );

    let router = ralleh_ai_router::AiRouter::with_backend_arc(backend);
    let request = completion_request("local", "desktop-1", "operator", "ping");
    let started = std::time::Instant::now();
    let outcome = router.route(&request).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    match outcome {
        CompletionOutcome::Succeeded(CompletionResponse { backend, text }) => {
            Ok(BackendTestResult {
                ok: true,
                backend,
                latency_ms: Some(elapsed_ms),
                sample_response: Some(truncate_preview(&text, 200)),
                error: None,
            })
        }
        CompletionOutcome::Failed { backend, error } => Ok(BackendTestResult {
            ok: false,
            backend,
            latency_ms: Some(elapsed_ms),
            sample_response: None,
            error: Some(error),
        }),
        CompletionOutcome::Denied => Ok(BackendTestResult {
            ok: false,
            backend: cfg.kind.label().to_string(),
            latency_ms: None,
            sample_response: None,
            error: Some("policy denied".into()),
        }),
        CompletionOutcome::ApprovalRequired => Ok(BackendTestResult {
            ok: false,
            backend: cfg.kind.label().to_string(),
            latency_ms: None,
            sample_response: None,
            error: Some("policy requires approval".into()),
        }),
        CompletionOutcome::NoBackendConfigured => Ok(BackendTestResult {
            ok: false,
            backend: cfg.kind.label().to_string(),
            latency_ms: None,
            sample_response: None,
            error: Some("no backend configured".into()),
        }),
    }
}

fn truncate_preview(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// Persist a completion config through the settings file and
/// hot-swap the router to use it. `None` clears the persisted
/// config, restoring env-var priority. Returns the new backend
/// status so the UI can update in place without a follow-up
/// `backend_status` call.
#[tauri::command]
fn assistant_save_backend(
    app: AppHandle,
    state: State<'_, AssistantState>,
    audit_log: State<'_, AuditLog>,
    config: Option<CompletionConfigUpdate>,
) -> Result<BackendStatus, String> {
    let store = open_default_secret_store();
    // Capture what the config *asked* for so audit events can
    // record the intent even when write_completion_config errors
    // out before persisting anything. `None` means "clear".
    let intent = config
        .as_ref()
        .map(|c| (c.kind, c.base_url.clone(), c.api_key.clone()));
    let previous_backend = state.current_backend_name();
    let settings = match write_completion_config(&app, store.as_ref(), config) {
        Ok(s) => s,
        Err(err) => {
            if looks_like_egress_denial(&err) {
                let (subject_kind, subject_url) = intent
                    .as_ref()
                    .map(|(k, u, _)| (k.label().to_string(), u.clone()))
                    .unwrap_or_default();
                audit_record(
                    &audit_log,
                    audit_event_with_identity(&app, AuditKind::EgressDeny)
                        .with_subject(format!("{subject_kind}@{subject_url}"))
                        .with_detail(serde_json::json!({
                            "stage": "save",
                            "reason": err,
                        })),
                );
            }
            return Err(err);
        }
    };
    // Save succeeded -- emit the appropriate secret-mutation
    // events. `Set` writes a new key, `Clear` removes one, `Keep`
    // is a no-op on the store and doesn't need an audit line.
    if let Some((kind, _url, key_update)) = intent.as_ref() {
        match key_update {
            ApiKeyUpdate::Set { value } if !value.is_empty() => {
                audit_record(
                    &audit_log,
                    audit_event_with_identity(&app, AuditKind::SecretWrite)
                        .with_subject("completion-api-key")
                        .with_detail(serde_json::json!({
                            "kind": SecretKind::CompletionApiKey,
                            "backend": kind.label(),
                        })),
                );
            }
            ApiKeyUpdate::Clear => {
                audit_record(
                    &audit_log,
                    audit_event_with_identity(&app, AuditKind::SecretClear)
                        .with_subject("completion-api-key")
                        .with_detail(serde_json::json!({
                            "kind": SecretKind::CompletionApiKey,
                            "backend": kind.label(),
                        })),
                );
            }
            _ => {}
        }
    }
    let active_backend = match settings.completion.as_ref() {
        Some(cfg) => {
            // Backend construction needs the raw key -- resolve it
            // from the store (post-save, this is authoritative) and
            // hand the resolved config to the router.
            let resolved = cfg.resolve_with_store(store.as_ref());
            state.reconfigure(&resolved)
        }
        None => {
            // Cleared: restart from env priority. We do NOT restart
            // the whole AssistantState -- swapping the backend
            // preserves the in-flight counter, gateway registry,
            // and policy engine, all of which live outside the
            // completion config.
            let fallback_cfg = settings::CompletionConfig {
                kind: settings::CompletionKind::Echo,
                ..Default::default()
            };
            state.reconfigure(&fallback_cfg)
        }
    };
    // Router swap: record the transition so a review can prove
    // which backend was actually active at any given moment.
    if active_backend != previous_backend {
        audit_record(
            &audit_log,
            audit_event_with_identity(&app, AuditKind::BackendSwap)
                .with_subject("router")
                .with_detail(serde_json::json!({
                    "from": previous_backend,
                    "to": active_backend,
                })),
        );
    }
    // Save path: return the current health snapshot without
    // forcing a probe. If the operator hit "test" first, the
    // snapshot already reflects that; if they didn't, the
    // background probe will refresh it within one interval.
    let health_snapshot = {
        let health: State<'_, assistant_health::Health> = app.state();
        let guard = health.lock().map_err(|e| e.to_string())?;
        guard.materialize(std::time::Instant::now())
    };
    Ok(BackendStatus {
        active_backend,
        configured: settings
            .completion
            .as_ref()
            .map(|c| RedactedCompletionConfig::from_config_and_store(c, store.as_ref())),
        health: health_snapshot,
    })
}

/// Return the last `limit` audit events. Clamped to `[1, 500]` so
/// the settings-UI diagnostic panel can't ask for so much data
/// it stalls the IPC bridge. `500` is generous — the panel
/// typically wants "the last dozen" — but bounded.
#[tauri::command]
fn assistant_audit_tail(
    audit_log: State<'_, AuditLog>,
    limit: Option<usize>,
) -> Result<Vec<AuditEvent>, String> {
    let n = limit.unwrap_or(50).clamp(1, 500);
    audit_log.tail(n)
}

/// Walk the audit log's hash chain and report any tampering
/// the verifier detected. Empty `issues` means the chain is
/// consistent with on-disk content right now; a non-empty list
/// names the specific lines and failure mode. See
/// `audit::AuditVerifyReport` for the caveat that this is
/// tamper-evident, not tamper-proof.
#[tauri::command]
fn assistant_audit_verify(audit: State<'_, AuditLog>) -> Result<audit::AuditVerifyReport, String> {
    audit.verify()
}

/// Assemble a diagnostics bundle (settings redacted, backend
/// status, health snapshot, audit tail, presence log tail)
/// and write it to a JSON file the operator can attach to a
/// support ticket. `dest_dir` overrides the default location
/// (the app config dir); the frontend passes `null` to accept
/// the default. Returns the absolute path of the file
/// written.
#[tauri::command]
fn assistant_diagnostics_bundle(
    app: AppHandle,
    dest_dir: Option<String>,
) -> Result<String, String> {
    let dest = dest_dir.map(std::path::PathBuf::from);
    let path = diagnostics::build_and_write(&app, dest)?;
    Ok(path.display().to_string())
}

/// Reveal `path` in the operator's OS file manager (Explorer,
/// Finder, or the freedesktop `xdg-open` on the parent
/// directory). Used by the "Show in file manager" affordance
/// on the diagnostics bundle button so the operator can drag
/// the file into a support ticket without hunting for the
/// path.
///
/// The path MUST be one this shell just produced (a
/// diagnostics bundle path or the app config dir). We validate
/// this by requiring `path` to be canonicalized and to sit
/// under the app config dir -- refusing arbitrary paths from
/// the webview closes the "webview asks the shell to reveal
/// /etc/shadow" trivial escalation. In production this is a
/// belt-and-braces check on top of the fact that the webview
/// only ever passes back paths *we* handed it, but the guard
/// is cheap and the failure mode is worth pre-empting.
#[tauri::command]
fn reveal_path_in_file_manager(app: AppHandle, path: String) -> Result<(), String> {
    use std::path::PathBuf;
    let target = PathBuf::from(&path)
        .canonicalize()
        .map_err(|e| format!("canonicalize {path}: {e}"))?;
    let allowed_root = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir: {e}"))?
        .canonicalize()
        .map_err(|e| format!("canonicalize config dir: {e}"))?;
    if !target.starts_with(&allowed_root) {
        return Err(format!(
            "reveal refused: {} is not under {}",
            target.display(),
            allowed_root.display()
        ));
    }
    // Platform-specific reveal. On Windows `explorer /select,`
    // opens the containing folder with the target highlighted;
    // on macOS `open -R` does the same via Finder; on Linux
    // we fall back to opening the parent directory with
    // `xdg-open` because there's no portable "select this file"
    // affordance across desktop environments.
    #[cfg(target_os = "windows")]
    {
        // L5: `explorer` parses its own command line rather than a
        // clean argv, and the `/select,<path>` selector only works
        // as a single combined token — splitting it into two args
        // reliably breaks the highlight. Argument injection is
        // already precluded here: `target` was canonicalized and
        // proven to sit under the app config dir above, and
        // `Command` never routes through a shell, so the value is
        // both trusted and passed without shell interpretation.
        //
        // The real bug worth fixing is that `canonicalize` yields a
        // `\\?\` extended-length prefix on Windows that `explorer`
        // won't accept for `/select`; strip it so the highlight
        // actually lands on the file instead of silently opening a
        // default folder.
        let display = target.to_string_lossy();
        let native = display.strip_prefix(r"\\?\").unwrap_or(display.as_ref());
        std::process::Command::new("explorer")
            .arg(format!("/select,{native}"))
            .spawn()
            .map_err(|e| format!("explorer: {e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&target)
            .spawn()
            .map_err(|e| format!("open: {e}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let parent = target.parent().unwrap_or(&target);
        std::process::Command::new("xdg-open")
            .arg(parent)
            .spawn()
            .map_err(|e| format!("xdg-open: {e}"))?;
    }
    Ok(())
}

/// Tail the presence-runtime stderr capture. Useful for triage
/// after the audit log shows a `presence-stalled` event —
/// callers pass the same `limit` shape (default 100, clamped
/// 1..=1000).
#[tauri::command]
fn presence_log_tail(
    presence_log: State<'_, std::sync::Arc<presence_log::PresenceLog>>,
    limit: Option<usize>,
) -> Result<Vec<String>, String> {
    let n = limit.unwrap_or(100).clamp(1, 1000);
    presence_log.tail(n)
}

#[tauri::command]
fn presence_mic_stop(pump: State<'_, MicPumpState>) -> Result<PresenceMicStatus, String> {
    let mut guard = pump.lock().map_err(|e| e.to_string())?;
    if let Some(mut p) = guard.take() {
        p.stop();
    }
    Ok(PresenceMicStatus {
        running: false,
        mic_feature: mic_feature_enabled(),
    })
}

/// Builds the reverse-channel listener that persists window geometry
/// to `EdgeSettings`. Runs on the presence reader thread — kept small
/// so a stream of `Moved` events during a drag does not stall the
/// runtime. Failure to persist is logged; it must never bubble up.
fn presence_event_listener(app: AppHandle) -> EventListener {
    Box::new(move |event| match event {
        // `PresenceEvent` is `#[non_exhaustive]` on the wire crate,
        // so a wildcard is required. Any new event variant added
        // later will fall through until it is explicitly handled —
        // ignoring an unknown event is safer than crashing the shell
        // on a runtime that speaks a newer wire.
        PresenceEvent::Ready { x, y } | PresenceEvent::Moved { x, y } => {
            // (0, 0) is the sentinel the runtime uses to mean "window
            // manager did not tell us the position" (e.g. Wayland).
            // Persisting it would clobber a real value from a previous
            // session, so we skip.
            if x == 0 && y == 0 {
                return;
            }
            let mut settings = match load_settings(&app) {
                Ok(s) => s,
                Err(err) => {
                    log::warn!(
                        "desktop-edge: presence position not persisted \
                         (load_settings failed: {err})"
                    );
                    return;
                }
            };
            let next = PresencePosition { x, y };
            if settings.presence_position == Some(next) {
                // No-op writes are wasteful on a drag stream.
                return;
            }
            settings.presence_position = Some(next);
            if let Err(err) = save_settings(&app, &settings) {
                log::warn!(
                    "desktop-edge: presence position not persisted \
                     (save_settings failed: {err})"
                );
            }
        }
        _ => {
            log::debug!("desktop-edge: ignoring unknown presence event");
        }
    })
}

/// Sends every persisted presence preference back to the runtime
/// right after spawn. Runs once, after `manage(presence)`, so a fresh
/// child receives the shell's view of "what the user last chose"
/// before the first frame lands. Every field is optional on the
/// settings side; missing values mean "use the runtime's default"
/// and skip the corresponding command.
///
/// The reduced-motion field is always sent because it is a plain
/// `bool` — sending `false` on a fresh install matches the runtime
/// default and costs one envelope, which is cheaper than tracking
/// an "explicitly set" sentinel.
/// Sparse scan sweep (§3.4). Optionally fires a short attention
/// pulse at a fixed interval, but only when `AssistantState` reports
/// zero in-flight work — so it never competes with real activity.
///
/// Opt-in via `RALLEH_SCAN_SWEEP_MS` (interval in milliseconds).
/// Missing / unparseable / zero disables the sweep entirely, which
/// is the default: firing an attention pulse on a fresh dev build
/// with no operator context would train the eye to ignore attention
/// events, which is the exact opposite of what sparse means.
///
/// A minimum interval of 5000 ms is enforced to keep the visual
/// language honest — a scan sweep is not a heartbeat.
fn spawn_scan_sweep(
    presence: &Presence,
    in_flight: std::sync::Arc<std::sync::atomic::AtomicUsize>,
) {
    const SWEEP_ENV: &str = "RALLEH_SCAN_SWEEP_MS";
    const MIN_INTERVAL_MS: u64 = 5_000;
    const PULSE_MS: u64 = 350;

    let interval_ms = match std::env::var(SWEEP_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        Some(0) | None => return,
        Some(v) => v.max(MIN_INTERVAL_MS),
    };

    // The sweep only needs a `Sender<Envelope>` to fire pulses, but
    // `Presence::pulse_attention` already knows how to spawn its own
    // detached release thread — so we clone the whole `Presence` by
    // pulling out its sender through a public shim. Rather than
    // widen the API for one caller, use the existing `pulse_attention`
    // method by cloning the `Presence` reference via a lightweight
    // handle: `Presence` itself is not Clone, so instead we grab a
    // `Sender<Envelope>` and reconstruct the two-envelope sequence
    // here. This keeps `Presence` opaque to the scan sweep.
    let Some(tx) = presence.sender_clone() else {
        // Presence disabled — nothing to sweep against.
        log::debug!("scan sweep skipped: presence disabled");
        return;
    };

    log::info!(
        "scan sweep enabled every {}ms (min {}ms enforced)",
        interval_ms,
        MIN_INTERVAL_MS
    );

    std::thread::Builder::new()
        .name("presence-scan-sweep".into())
        .spawn(move || {
            use presence_ipc::{Command, Envelope};
            let interval = std::time::Duration::from_millis(interval_ms);
            let pulse_hold = std::time::Duration::from_millis(PULSE_MS);
            loop {
                std::thread::sleep(interval);
                if in_flight.load(std::sync::atomic::Ordering::Acquire) != 0 {
                    // Something real is happening — skip this beat
                    // rather than layering attention on top of it.
                    continue;
                }
                let engage = Envelope::wrap(Command::SetMode {
                    mode: presence_ipc::PresenceMode::Attention,
                    engaged: true,
                });
                if tx.send(engage).is_err() {
                    log::info!("scan sweep exiting: presence pipe closed");
                    return;
                }
                std::thread::sleep(pulse_hold);
                let release = Envelope::wrap(Command::SetMode {
                    mode: presence_ipc::PresenceMode::Attention,
                    engaged: false,
                });
                if tx.send(release).is_err() {
                    return;
                }
            }
        })
        .expect("spawn presence-scan-sweep thread");
}

fn restore_presence_state(app: &AppHandle, presence: &Presence) {
    let Ok(settings) = load_settings(app) else {
        return;
    };
    if let Some(pos) = settings.presence_position {
        presence.send(PresenceCommand::SetPosition { x: pos.x, y: pos.y });
    }
    if let Some(palette) = settings.presence_palette {
        presence.send(PresenceCommand::SetPalette { palette });
    }
    if let Some(tier) = settings.presence_quality_tier {
        presence.send(PresenceCommand::SetQualityTier { tier });
    }
    // Only send if the user opted in — sending `false` on every launch
    // would fire an unnecessary transition on the runtime.
    if settings.presence_reduced_motion {
        presence.send(PresenceCommand::SetReducedMotion { enabled: true });
    }
}

/// Load-modify-save helper for the persisted presence preferences.
/// Runs on the Tauri command thread, which is fine — Tauri already
/// serializes command invocations for us, and the file writes are
/// small enough that a slow disk still lands within one frame.
/// Errors are logged rather than surfaced; a settings write failure
/// must not fail the visual command that triggered it, because the
/// user has no way to correct it and the visual side already
/// happened.
fn update_presence_settings<F>(app: &AppHandle, mutate: F)
where
    F: FnOnce(&mut EdgeSettings),
{
    let mut settings = match load_settings(app) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("desktop-edge: presence preference not persisted (load: {err})");
            return;
        }
    };
    mutate(&mut settings);
    if let Err(err) = save_settings(app, &settings) {
        log::warn!("desktop-edge: presence preference not persisted (save: {err})");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mic_pump: MicPumpState = Mutex::new(None);
    // Assistant state is constructed with env-priority defaults here;
    // the persisted UI config (if any) is applied inside `setup` once
    // an `AppHandle` is available to load the settings file. This
    // means a first-launch shell (no settings file yet) uses whatever
    // `RALLEH_COMPLETION_*` env vars are set, matching the pre-UI
    // behaviour, and a returning-user shell settles into the UI-owned
    // config within a few dozen ms of startup.
    let assistant_state = AssistantState::with_defaults(None);
    let assistant_in_flight = assistant_state.in_flight_handle();

    // Best-effort open of the default audio output. `try_open_default`
    // soft-skips under CI or when `RALLEH_SKIP_LIVE_AUDIO` is set, and
    // returns `None` on any hardware failure -- the shell should stay
    // launchable on hosts without an output device, just without
    // audible TTS. `voice_smoke` guards on this being `Some` before
    // enqueueing.
    #[cfg(feature = "playback")]
    let playback_sink: PlaybackSinkState = Mutex::new(CpalPlaybackSink::try_open_default());

    let builder = tauri::Builder::default()
        .manage(mic_pump)
        .manage(assistant_state);
    #[cfg(feature = "playback")]
    let builder = builder.manage(playback_sink);
    builder
        // Presence is spawned inside `setup` so the reverse-channel
        // listener can capture an `AppHandle` — we need one to load
        // and save `EdgeSettings` from the reader thread. Everything
        // else about the previous lifecycle carries over: on shutdown
        // Tauri drops the managed `Presence`, which closes stdin and
        // kills the child (see `presence::Presence::drop`).
        .setup(move |app| {
            let handle = app.handle().clone();
            // Audit log opens against the OS app config dir. On
            // failure we fall back to the no-op sink so a broken
            // audit surface never blocks the shell from starting —
            // fail-open is the intended posture (see audit.rs
            // module docs). The warning line names the reason so
            // an operator can spot it in the log.
            let audit_log = match AuditLog::open(&handle) {
                Ok(log) => {
                    log::info!("audit: recording to {}", log.active_path().display());
                    log
                }
                Err(e) => {
                    log::warn!("audit: opening log failed: {e}; using no-op sink");
                    AuditLog::no_op()
                }
            };
            app.manage(audit_log);
            // Settings schema migration: run once, before any
            // load path reads the file. `AlreadyCurrent` /
            // `NoFile` are silent (no audit spam on the happy
            // path); every other outcome writes an audit event
            // so an operator can prove which shell touched this
            // profile last.
            let migration_state: tauri::State<'_, AuditLog> = app.state();
            match migrate_settings_file(&handle) {
                SettingsMigrationOutcome::Migrated {
                    from,
                    to,
                    backup_path,
                } => {
                    log::info!("settings: migrated edge-settings.json from v{from} to v{to}");
                    audit_record(
                        &migration_state,
                        audit_event_with_identity(&handle, AuditKind::SettingsMigrate)
                            .with_subject("edge-settings.json")
                            .with_detail(serde_json::json!({
                                "from": from,
                                "to": to,
                                "backup_path": backup_path
                                    .as_ref()
                                    .map(|p| p.display().to_string()),
                            })),
                    );
                }
                SettingsMigrationOutcome::AlreadyCurrent | SettingsMigrationOutcome::NoFile => {}
                SettingsMigrationOutcome::FutureVersion { on_disk } => {
                    log::warn!(
                        "settings: on-disk version {on_disk} newer than this build; \
                         running from defaults, file untouched"
                    );
                    audit_record(
                        &migration_state,
                        audit_event_with_identity(&handle, AuditKind::SettingsMigrateFailed)
                            .with_subject("edge-settings.json")
                            .with_detail(serde_json::json!({
                                "reason": "future-version",
                                "on_disk": on_disk,
                                "supported_up_to": settings::CURRENT_SETTINGS_VERSION,
                            })),
                    );
                }
                SettingsMigrationOutcome::RewriteFailed { from, reason } => {
                    log::warn!(
                        "settings: migrated v{from} -> \
                         v{cur} in memory but rewrite failed: {reason}",
                        cur = settings::CURRENT_SETTINGS_VERSION
                    );
                    audit_record(
                        &migration_state,
                        audit_event_with_identity(&handle, AuditKind::SettingsMigrateFailed)
                            .with_subject("edge-settings.json")
                            .with_detail(serde_json::json!({
                                "reason": reason,
                                "from": from,
                                "stage": "rewrite",
                            })),
                    );
                }
                SettingsMigrationOutcome::Unreadable { reason } => {
                    log::warn!(
                        "settings: cannot read edge-settings.json ({reason}); \
                         running from defaults"
                    );
                    audit_record(
                        &migration_state,
                        audit_event_with_identity(&handle, AuditKind::SettingsMigrateFailed)
                            .with_subject("edge-settings.json")
                            .with_detail(serde_json::json!({
                                "reason": reason,
                                "stage": "load",
                            })),
                    );
                }
            }
            // Router health probe. Starts in the Unknown state
            // until the first probe returns; edge transitions
            // (Healthy <-> Unhealthy) emit audit events. The
            // background thread reads AssistantState fresh on
            // every tick so a mid-session backend swap is picked
            // up automatically. Skipped entirely (state stays
            // Unknown -> Skipped) when disabled by env var.
            let health: assistant_health::Health =
                std::sync::Arc::new(std::sync::Mutex::new(assistant_health::HealthInner::new(
                    app.state::<AssistantState>().current_backend_name(),
                )));
            app.manage(health.clone());
            assistant_health::spawn(handle.clone(), health);
            // Capture the runtime's stderr into a rotated file so
            // a stall event has a correlation trail. Open before
            // spawn so the sink is available for the first line
            // the runtime writes.
            let presence_log = std::sync::Arc::new(presence_log::PresenceLog::open(&handle));
            if let Some(path) = presence_log.active_path() {
                log::info!(
                    "presence-log: capturing runtime stderr to {}",
                    path.display()
                );
            }
            let log_sink = presence_log.clone();
            let stderr_sink: presence::StderrSink = std::sync::Arc::new(move |line: &str| {
                if let Err(e) = log_sink.write_line(line) {
                    // Fail-open: never lose a line just because
                    // the file is unavailable. `debug` (not
                    // `warn`) because a broken log while stderr
                    // is high-volume would otherwise flood the
                    // shell's own log.
                    log::debug!("presence-log: write failed ({e}); line: {line}");
                }
            });
            app.manage(presence_log);
            let presence = Presence::spawn_from_env_with_log(
                presence_event_listener(handle.clone()),
                Some(stderr_sink),
            );
            restore_presence_state(&handle, &presence);
            // Liveness monitor. Reads the same `Liveness` handle the
            // stdout reader stamps on each incoming event, and
            // records `PresenceStalled` / `PresenceRecovered` audit
            // events on edges. Exits automatically when presence is
            // disabled (no spawn timestamp) — see
            // `presence_health::spawn`.
            presence_health::spawn(handle.clone(), presence.liveness_handle());
            // Scan sweep (§3.4). Opt-in via env var so a normal
            // launch stays silent — the visual grammar treats
            // attention as *sparse*, and firing it on a fresh dev
            // build would train the operator to ignore it.
            spawn_scan_sweep(&presence, assistant_in_flight.clone());
            // Apply persisted completion config, if any. Env-var
            // configuration remains the source of truth when
            // `settings.completion` is `None`, so an operator can
            // still drop-in override with `RALLEH_COMPLETION_*` on
            // a first-launch shell.
            if let Ok(mut settings) = load_settings(&handle) {
                let store = open_default_secret_store();
                // One-shot migration: any cleartext api_key still on
                // disk from an older shell version is transferred
                // into the OS keychain (when available) and cleared
                // from the JSON. Best-effort — on a host without a
                // keychain the fallback path keeps working with the
                // cleartext copy, and the UI surfaces the insecure
                // storage via the `storage` field.
                let audit_state: tauri::State<'_, AuditLog> = app.state();
                match migrate_completion_secret(&handle, store.as_ref(), &mut settings) {
                    Ok(true) => {
                        log::info!("assistant: migrated cleartext API key to OS keychain");
                        audit_record(
                            &audit_state,
                            audit_event_with_identity(&handle, AuditKind::SecretMigrate)
                                .with_subject("completion-api-key")
                                .with_detail(serde_json::json!({
                                    "kind": SecretKind::CompletionApiKey,
                                })),
                        );
                    }
                    Ok(false) => {}
                    Err(e) => {
                        log::warn!(
                            "assistant: keychain migration deferred ({e}); \
                             continuing with cleartext key on disk"
                        );
                        audit_record(
                            &audit_state,
                            audit_event_with_identity(&handle, AuditKind::SecretMigrateFailed)
                                .with_subject("completion-api-key")
                                .with_detail(serde_json::json!({
                                    "reason": e.to_string(),
                                })),
                        );
                    }
                }
                if let Some(cfg) = settings.completion.as_ref() {
                    let resolved = cfg.resolve_with_store(store.as_ref());
                    let state: tauri::State<'_, AssistantState> = app.state();
                    let before = state.current_backend_name();
                    let name = state.reconfigure(&resolved);
                    log::info!("assistant: startup reconfigure → {name}");
                    if name != before {
                        audit_record(
                            &audit_state,
                            audit_event_with_identity(&handle, AuditKind::BackendSwap)
                                .with_subject("router")
                                .with_detail(serde_json::json!({
                                    "from": before,
                                    "to": name,
                                    "trigger": "startup",
                                })),
                        );
                    }
                }
            }
            app.manage(presence);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            core_ping,
            voice_smoke,
            clipboard_smoke,
            mic_smoke,
            load_edge_settings,
            save_edge_settings,
            edge_settings_path,
            presence_status,
            presence_set_mode,
            presence_set_signals,
            presence_set_reduced_motion,
            presence_apply_reduced_motion,
            presence_current_modes,
            presence_set_palette,
            presence_set_ring_wanted,
            presence_set_quality_tier,
            presence_set_interactive,
            presence_mic_status,
            presence_mic_start,
            presence_mic_stop,
            assistant_think,
            assistant_think_stream,
            assistant_tool_ping,
            assistant_notify_inbound,
            assistant_backend_status,
            assistant_test_backend,
            assistant_save_backend,
            assistant_audit_tail,
            assistant_audit_verify,
            assistant_diagnostics_bundle,
            assistant_probe_backend,
            presence_log_tail,
            reveal_path_in_file_manager,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
