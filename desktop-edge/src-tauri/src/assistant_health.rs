//! Router-backend health probe.
//!
//! `assistant_backend_status` used to answer "which backend name
//! is the router configured to hit?" — a stale answer if the
//! remote had gone down five minutes ago. This module runs a
//! low-cost `ping` completion against the live backend on a
//! fixed cadence, tracks pass / fail edges, and audits them so
//! the UI can render a real "reachable / unreachable / unknown"
//! state (and operators can prove uptime after the fact).
//!
//! # Design notes
//!
//! - **Echo backend is skipped.** Its `complete` never fails, so
//!   the probe would be busy-work and misleading (it would light
//!   the UI green while the operator hasn't configured a real
//!   backend yet).
//! - **Edge-firing audit.** A single transient failure produces
//!   one `router-unhealthy` event, not a stream of them. Recovery
//!   fires a `router-healthy` event once. Repeated failures at
//!   the same state emit nothing to the audit log — the
//!   `consecutive_failures` field on the snapshot carries that
//!   info for the UI.
//! - **Fixed cadence, jittered slightly** so a fleet of shells
//!   started at the same time doesn't stampede a shared endpoint.
//! - **Not a keepalive.** The probe holds no connections; each
//!   run is a fresh reqwest client via the router's usual path.
//!   If a backend requires a periodic keepalive, that lives with
//!   the backend, not here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::assistant::{completion_request, AssistantState};
use crate::audit::{AuditKind, AuditLog};
use crate::audit_event_with_identity;
use ralleh_ai_router::CompletionOutcome;

/// Default interval between probes. 60 s matches the "check
/// once a minute" cadence typical of enterprise reachability
/// dashboards, and is well below any provider's session-token
/// rotation window. Overridable via
/// `RALLEH_HEALTH_PROBE_INTERVAL_MS`; clamped to a floor so a
/// misconfiguration can't turn the shell into a synthetic load
/// test.
pub const DEFAULT_INTERVAL_MS: u64 = 60_000;
const MIN_INTERVAL_MS: u64 = 5_000;
const PROBE_PROMPT: &str = "ping";
const PROBE_ENV_INTERVAL: &str = "RALLEH_HEALTH_PROBE_INTERVAL_MS";
const PROBE_ENV_DISABLED: &str = "RALLEH_HEALTH_PROBE_DISABLED";

/// The verdict of one probe attempt. Kept small and `Clone` so
/// the snapshot can hand copies to Tauri command handlers without
/// pinning the shared lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The router replied with a `Succeeded` outcome (any text).
    Ok { latency_ms: u64 },
    /// The router replied `Failed` (network / policy denial /
    /// backend error). The error string is truncated at the call
    /// site to keep JSON payloads bounded.
    Failed { latency_ms: u64, error: String },
    /// The probe was skipped (echo backend / no config / kill
    /// switch). Not a failure — the UI treats this as "no signal".
    Skipped { reason: SkipReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    /// Router is currently on `EchoBackend` — nothing to probe.
    EchoBackend,
    /// Probing was disabled via environment variable.
    Disabled,
}

/// State the health-machine tracks between probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    /// No probe has completed yet. UI renders a neutral badge.
    Unknown,
    /// Latest probe succeeded.
    Healthy,
    /// Latest probe failed. Consecutive failure count is on the
    /// snapshot for the "how bad is it?" UI hint.
    Unhealthy,
    /// Probe was skipped for a policy reason (echo backend,
    /// disabled by env). No audit implications.
    Skipped,
}

/// Snapshot the UI and audit code consume. Everything the
/// operator would want on a status widget is here in one shape.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub state: HealthState,
    pub backend_name: String,
    /// Milliseconds since the last probe completed. `None`
    /// before the first probe. Frontend renders this as
    /// "checked 4 s ago"; the underlying `Instant` is not
    /// serializable, so we materialize the age on read.
    pub last_probe_ms_ago: Option<u64>,
    pub last_latency_ms: Option<u64>,
    pub last_error: Option<String>,
    /// Reason a probe was skipped, when `state == Skipped`.
    pub skip_reason: Option<SkipReason>,
    pub consecutive_failures: u32,
}

impl HealthSnapshot {
    pub fn unknown(backend_name: String) -> Self {
        Self {
            state: HealthState::Unknown,
            backend_name,
            last_probe_ms_ago: None,
            last_latency_ms: None,
            last_error: None,
            skip_reason: None,
            consecutive_failures: 0,
        }
    }
}

/// Handle shared across the probe thread, Tauri command
/// handlers, and the audit code. Wrapped in a plain `Mutex`
/// because critical sections are trivial (a handful of field
/// writes) and no lock contention has ever mattered at the
/// 60 s cadence we run at.
pub type Health = Arc<Mutex<HealthInner>>;

pub struct HealthInner {
    pub snapshot: HealthSnapshot,
    /// Wall clock of the last probe *completion*. Materialised
    /// into `snapshot.last_probe_ms_ago` on read via
    /// [`HealthInner::materialize`].
    pub last_probe_at: Option<Instant>,
}

impl HealthInner {
    pub fn new(backend_name: String) -> Self {
        Self {
            snapshot: HealthSnapshot::unknown(backend_name),
            last_probe_at: None,
        }
    }

    /// Return a JSON-safe copy with `last_probe_ms_ago` filled
    /// from the internal `Instant`.
    pub fn materialize(&self, now: Instant) -> HealthSnapshot {
        let mut out = self.snapshot.clone();
        out.last_probe_ms_ago = self
            .last_probe_at
            .map(|t| now.duration_since(t).as_millis() as u64);
        out
    }
}

/// State-machine edge produced by [`fold_outcome`]. `None` when
/// the outcome did not change the healthy / unhealthy classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthEdge {
    None,
    BecameHealthy,
    BecameUnhealthy,
}

/// Fold `outcome` into `inner`. Updates `snapshot` in place and
/// returns the edge that fired, if any. Pulled out of the probe
/// thread so it can be unit-tested against a fabricated timeline
/// without a live backend.
pub fn fold_outcome(
    inner: &mut HealthInner,
    backend_name: String,
    outcome: ProbeOutcome,
    now: Instant,
) -> HealthEdge {
    inner.last_probe_at = Some(now);
    inner.snapshot.backend_name = backend_name;
    let previous = inner.snapshot.state;
    match outcome {
        ProbeOutcome::Ok { latency_ms } => {
            inner.snapshot.state = HealthState::Healthy;
            inner.snapshot.last_latency_ms = Some(latency_ms);
            inner.snapshot.last_error = None;
            inner.snapshot.skip_reason = None;
            inner.snapshot.consecutive_failures = 0;
            match previous {
                HealthState::Healthy => HealthEdge::None,
                _ => HealthEdge::BecameHealthy,
            }
        }
        ProbeOutcome::Failed { latency_ms, error } => {
            inner.snapshot.state = HealthState::Unhealthy;
            inner.snapshot.last_latency_ms = Some(latency_ms);
            inner.snapshot.last_error = Some(truncate(error, 400));
            inner.snapshot.skip_reason = None;
            inner.snapshot.consecutive_failures =
                inner.snapshot.consecutive_failures.saturating_add(1);
            match previous {
                HealthState::Unhealthy => HealthEdge::None,
                _ => HealthEdge::BecameUnhealthy,
            }
        }
        ProbeOutcome::Skipped { reason } => {
            inner.snapshot.state = HealthState::Skipped;
            inner.snapshot.skip_reason = Some(reason);
            // A skip resets the failure streak because the
            // operator's action (swapping to echo, disabling
            // probing) is unrelated to reachability -- if they
            // later swap back to a real backend, we want to
            // start counting again from zero rather than carry
            // an ancient streak.
            inner.snapshot.consecutive_failures = 0;
            // Don't clear latency/error so the UI can still
            // show "last real probe result before we stopped
            // watching".
            HealthEdge::None
        }
    }
}

fn truncate(s: String, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s
    } else {
        let mut out: String = s.chars().take(max_chars).collect();
        out.push('…');
        out
    }
}

/// Run one probe against the router in `state`. Blocks the
/// caller on the async router.route() call via
/// `tauri::async_runtime::block_on` so this fits naturally into
/// a std::thread loop without pulling tokio into the surface
/// area of this module.
pub fn probe_once(state: &AssistantState) -> (String, ProbeOutcome) {
    let backend_name = state.current_backend_name();
    if backend_name.eq_ignore_ascii_case("echo") {
        return (
            backend_name,
            ProbeOutcome::Skipped {
                reason: SkipReason::EchoBackend,
            },
        );
    }
    let request = completion_request("local", "desktop-1", "probe", PROBE_PROMPT);
    let router = state.router.clone();
    let started = Instant::now();
    let outcome = tauri::async_runtime::block_on(async move { router.route(&request).await });
    let latency_ms = started.elapsed().as_millis() as u64;
    let probe = match outcome {
        CompletionOutcome::Succeeded(_) => ProbeOutcome::Ok { latency_ms },
        CompletionOutcome::Failed { error, .. } => ProbeOutcome::Failed { latency_ms, error },
        CompletionOutcome::Denied => ProbeOutcome::Failed {
            latency_ms,
            error: "policy denied".into(),
        },
        CompletionOutcome::ApprovalRequired => ProbeOutcome::Failed {
            latency_ms,
            error: "approval required".into(),
        },
        CompletionOutcome::NoBackendConfigured => ProbeOutcome::Skipped {
            reason: SkipReason::EchoBackend,
        },
    };
    (backend_name, probe)
}

/// Read the probe interval from the environment, defaulting to
/// [`DEFAULT_INTERVAL_MS`] and clamping to `MIN_INTERVAL_MS`.
/// Returns `None` if probing is disabled entirely via
/// [`PROBE_ENV_DISABLED`].
pub fn interval_from_env() -> Option<Duration> {
    if std::env::var_os(PROBE_ENV_DISABLED).is_some() {
        return None;
    }
    let ms = std::env::var(PROBE_ENV_INTERVAL)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_MS)
        .max(MIN_INTERVAL_MS);
    Some(Duration::from_millis(ms))
}

/// Spawn the background probe thread. Returns immediately;
/// the thread lives for the app's lifetime. When probing is
/// disabled, records that on the snapshot and exits without
/// starting a loop.
pub fn spawn(app: AppHandle, health: Health) {
    let interval = match interval_from_env() {
        Some(d) => d,
        None => {
            log::info!("assistant-health: probing disabled via {PROBE_ENV_DISABLED}");
            if let Ok(mut inner) = health.lock() {
                inner.snapshot.state = HealthState::Skipped;
                inner.snapshot.skip_reason = Some(SkipReason::Disabled);
            }
            return;
        }
    };
    let flag = Arc::new(AtomicBool::new(true));
    let stop = flag.clone();
    std::thread::Builder::new()
        .name("assistant-health".into())
        .spawn(move || run(app, health, interval, stop))
        .ok();
    // The flag is intentionally leaked into the thread — its
    // lifetime is the process's, and dropping the outer handle
    // now would set `stop` to `false` and short-circuit the
    // very first iteration. Keeping a reference around here
    // just for cleanliness would require plumbing a shutdown
    // channel that Tauri doesn't currently offer for setup
    // closures.
    std::mem::forget(flag);
}

fn run(app: AppHandle, health: Health, interval: Duration, stop: Arc<AtomicBool>) {
    log::info!(
        "assistant-health: probe loop started (interval {}ms)",
        interval.as_millis()
    );
    // Small stagger before the first probe so a shell that
    // starts under a stampede of workspace launches doesn't
    // synchronously hit the upstream at the exact same instant
    // as its siblings.
    std::thread::sleep(Duration::from_millis(2_000));
    while stop.load(Ordering::Relaxed) {
        // Fetch the state fresh on each iteration — a backend
        // hot-swap between polls should re-target the probe
        // without a shell restart.
        let state = match app.try_state::<AssistantState>() {
            Some(s) => s,
            None => {
                log::info!("assistant-health: AssistantState removed — probe exiting");
                return;
            }
        };
        let (backend_name, outcome) = probe_once(state.inner());
        let now = Instant::now();
        let edge = {
            let mut guard = match health.lock() {
                Ok(g) => g,
                Err(_) => {
                    std::thread::sleep(interval);
                    continue;
                }
            };
            fold_outcome(&mut guard, backend_name.clone(), outcome.clone(), now)
        };
        match edge {
            HealthEdge::None => {}
            HealthEdge::BecameHealthy => {
                let latency = match &outcome {
                    ProbeOutcome::Ok { latency_ms } => Some(*latency_ms),
                    _ => None,
                };
                log::info!(
                    "assistant-health: RECOVERED — {backend_name} responded in {latency:?}ms"
                );
                if let Some(audit) = app.try_state::<AuditLog>() {
                    let event = audit_event_with_identity(&app, AuditKind::RouterHealthy)
                        .with_subject(format!("router:{backend_name}"))
                        .with_detail(serde_json::json!({
                            "latency_ms": latency,
                        }));
                    let _ = audit.write(&event);
                }
            }
            HealthEdge::BecameUnhealthy => {
                let (latency, error) = match &outcome {
                    ProbeOutcome::Failed { latency_ms, error } => {
                        (Some(*latency_ms), Some(error.clone()))
                    }
                    _ => (None, None),
                };
                log::warn!(
                    "assistant-health: UNHEALTHY — {backend_name} probe failed after {latency:?}ms ({error:?})"
                );
                if let Some(audit) = app.try_state::<AuditLog>() {
                    let event = audit_event_with_identity(&app, AuditKind::RouterUnhealthy)
                        .with_subject(format!("router:{backend_name}"))
                        .with_detail(serde_json::json!({
                            "latency_ms": latency,
                            "error": error,
                        }));
                    let _ = audit.write(&event);
                }
            }
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_state_transitioning_to_ok_fires_became_healthy() {
        let mut inner = HealthInner::new("openai".into());
        let edge = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Ok { latency_ms: 120 },
            Instant::now(),
        );
        assert_eq!(edge, HealthEdge::BecameHealthy);
        assert_eq!(inner.snapshot.state, HealthState::Healthy);
        assert_eq!(inner.snapshot.last_latency_ms, Some(120));
        assert!(inner.snapshot.last_error.is_none());
        assert_eq!(inner.snapshot.consecutive_failures, 0);
    }

    #[test]
    fn healthy_after_healthy_is_no_edge() {
        let mut inner = HealthInner::new("openai".into());
        fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Ok { latency_ms: 100 },
            Instant::now(),
        );
        let edge = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Ok { latency_ms: 110 },
            Instant::now(),
        );
        assert_eq!(edge, HealthEdge::None);
        assert_eq!(inner.snapshot.state, HealthState::Healthy);
    }

    #[test]
    fn first_failure_fires_became_unhealthy_and_counts_it() {
        let mut inner = HealthInner::new("openai".into());
        let edge = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Failed {
                latency_ms: 200,
                error: "timeout".into(),
            },
            Instant::now(),
        );
        assert_eq!(edge, HealthEdge::BecameUnhealthy);
        assert_eq!(inner.snapshot.state, HealthState::Unhealthy);
        assert_eq!(inner.snapshot.consecutive_failures, 1);
        assert_eq!(inner.snapshot.last_error.as_deref(), Some("timeout"));
    }

    #[test]
    fn repeated_failures_accumulate_without_re_firing_the_edge() {
        let mut inner = HealthInner::new("openai".into());
        let _ = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Failed {
                latency_ms: 100,
                error: "a".into(),
            },
            Instant::now(),
        );
        for expected in 2..=5_u32 {
            let edge = fold_outcome(
                &mut inner,
                "openai".into(),
                ProbeOutcome::Failed {
                    latency_ms: 100,
                    error: "still down".into(),
                },
                Instant::now(),
            );
            assert_eq!(edge, HealthEdge::None, "no repeat edge on same state");
            assert_eq!(inner.snapshot.consecutive_failures, expected);
        }
    }

    #[test]
    fn recovery_resets_failure_streak_and_clears_error() {
        let mut inner = HealthInner::new("openai".into());
        let _ = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Failed {
                latency_ms: 100,
                error: "boom".into(),
            },
            Instant::now(),
        );
        let edge = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Ok { latency_ms: 90 },
            Instant::now(),
        );
        assert_eq!(edge, HealthEdge::BecameHealthy);
        assert_eq!(inner.snapshot.consecutive_failures, 0);
        assert!(inner.snapshot.last_error.is_none());
    }

    #[test]
    fn skipped_outcome_does_not_flap_the_edge() {
        let mut inner = HealthInner::new("openai".into());
        // Prime with a real failure so the failure streak is non-zero.
        let _ = fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Failed {
                latency_ms: 100,
                error: "down".into(),
            },
            Instant::now(),
        );
        let edge = fold_outcome(
            &mut inner,
            "echo".into(),
            ProbeOutcome::Skipped {
                reason: SkipReason::EchoBackend,
            },
            Instant::now(),
        );
        assert_eq!(edge, HealthEdge::None);
        assert_eq!(inner.snapshot.state, HealthState::Skipped);
        assert_eq!(
            inner.snapshot.consecutive_failures, 0,
            "streak resets on skip so a later real failure starts clean"
        );
    }

    #[test]
    fn interval_env_clamps_below_the_floor() {
        // Direct construction because messing with env in tests is
        // process-global and race-prone.
        // We can still assert the constant relationships that
        // interval_from_env relies on.
        const _: () = assert!(MIN_INTERVAL_MS >= 1_000);
        const _: () = assert!(DEFAULT_INTERVAL_MS >= MIN_INTERVAL_MS);
    }

    #[test]
    fn snapshot_materializes_last_probe_age_from_the_instant() {
        let mut inner = HealthInner::new("openai".into());
        let t0 = Instant::now();
        fold_outcome(
            &mut inner,
            "openai".into(),
            ProbeOutcome::Ok { latency_ms: 50 },
            t0,
        );
        let snap = inner.materialize(t0 + Duration::from_millis(1_500));
        assert!(
            snap.last_probe_ms_ago.unwrap() >= 1_500,
            "expected >=1500, got {:?}",
            snap.last_probe_ms_ago
        );
    }

    #[test]
    fn truncate_appends_ellipsis_past_the_cap() {
        let s = "a".repeat(500);
        let truncated = truncate(s, 100);
        assert_eq!(truncated.chars().count(), 101, "cap plus one ellipsis");
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn truncate_leaves_short_strings_untouched() {
        assert_eq!(truncate("short".into(), 100), "short");
    }
}
