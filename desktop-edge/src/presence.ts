// Typed wrappers around the Tauri commands defined in
// `src-tauri/src/lib.rs` (`presence_*`). Every value in this file is
// mirroring a `presence_ipc` type from `crates/presence-ipc/src/lib.rs`
// — if a wire spelling changes there, it changes here too. Kept as
// literal-union types rather than TS enums so a stale string surfaces
// as a compile error at the callsite, not at runtime.
//
// Phase 2 §3 of docs/PRESENCE_INTEGRATION_PLAN.md. The corresponding
// receiver is `SceneDirector::apply_command` in `presence-core`.

import { Channel, invoke, InvokeArgs } from "@tauri-apps/api/core";

/**
 * A visual mode the presence can be told is active. Additive on the
 * shell side — engaging `thinking` and `speaking` together does not
 * swap the shape, it raises both terms. See ADR-012.
 */
export type PresenceMode =
  | "thinking"
  | "speaking"
  | "tool_use"
  | "listening"
  | "attention"
  | "error";

export type QualityTier = "balanced" | "low";

export type PaletteId = "teal" | "lime" | "ice" | "ember";

export type PresenceStatus = {
  /**
   * `true` iff `RALLEH_PRESENCE_BIN` was set at shell startup and the
   * child process was successfully spawned. When `false`, every
   * `presenceSet*` call below is a no-op on the Rust side — safe to
   * fire, will not throw, will not do anything.
   */
  enabled: boolean;
  /**
   * Milliseconds since the shell last observed any reverse-channel
   * event from the runtime. `null` before the first event or when
   * presence is disabled. Values above the stall threshold indicate
   * the runtime is not currently emitting — the audit log will
   * contain a `presence-stalled` event by that point.
   */
  last_event_ms_ago: number | null;
  /**
   * Most recent heartbeat sequence observed by the shell, or `null`
   * before the first heartbeat.
   */
  last_heartbeat_sequence: number | null;
  /**
   * Runtime `uptime_ms` reported on the last heartbeat, or `null`
   * before the first heartbeat.
   */
  last_heartbeat_uptime_ms: number | null;
};

// Every ipc caller in this file wraps `invoke` in a `try` so a
// misbehaving renderer cannot bubble an error into the UI. A visual
// signal that fails to send is exactly the case we would rather drop
// than surface — the user has no reason to see it, and there is no
// meaningful action they could take.
async function safeInvoke<T = void>(cmd: string, args?: InvokeArgs): Promise<T | null> {
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    console.warn(`presence: ${cmd} failed`, err);
    return null;
  }
}

export async function presenceStatus(): Promise<PresenceStatus> {
  const status = await safeInvoke<PresenceStatus>("presence_status");
  return (
    status ?? {
      enabled: false,
      last_event_ms_ago: null,
      last_heartbeat_sequence: null,
      last_heartbeat_uptime_ms: null,
    }
  );
}

export function presenceSetMode(mode: PresenceMode, engaged: boolean): Promise<void> {
  return safeInvoke("presence_set_mode", { mode, engaged }).then(() => undefined);
}

/**
 * Continuous signal packet. Wire-level shape matches
 * `presence_ipc::Signals` — the runtime treats `activeModes` as
 * authoritative (modes not in the list get released), so pass the
 * caller's currently-engaged set if you don't want to disturb them.
 * All scalars are clamped by the runtime to conservative ranges
 * (`intensity` to `[0.0, 1.5]`, the other two to `[0.0, 1.0]`), and
 * `NaN` is folded to the low bound — a misbehaving sender cannot
 * corrupt the simulation, only fail to command it.
 */
export type PresenceSignals = {
  intensity: number;
  audioLevel: number;
  progress: number;
  activeModes: PresenceMode[];
};

export function presenceSetSignals(signals: PresenceSignals): Promise<void> {
  return safeInvoke("presence_set_signals", { signals }).then(() => undefined);
}

/**
 * Live-mic pump status. `micFeature` is `false` on a shell built
 * without the `mic` Cargo feature — the pump can never start there, and
 * the UI should say why rather than silently doing nothing.
 */
export type PresenceMicStatus = {
  running: boolean;
  micFeature: boolean;
};

export async function presenceMicStatus(): Promise<PresenceMicStatus> {
  const s = await safeInvoke<PresenceMicStatus>("presence_mic_status");
  return s ?? { running: false, micFeature: false };
}

/**
 * Starts the live-mic pump. Fails synchronously (rejects) if the shell
 * has not had mic clearance stamped, if the presence runtime is not
 * spawned, or if the OS reports no default input device. The dev panel
 * surfaces the error so the operator can react.
 */
export async function presenceMicStart(): Promise<PresenceMicStatus> {
  // Cannot use safeInvoke here — a failure to start is *actionable*,
  // and swallowing it would leave the toggle button perpetually stuck
  // in "off" with no explanation.
  return (await invoke<PresenceMicStatus>("presence_mic_start")) as PresenceMicStatus;
}

export async function presenceMicStop(): Promise<PresenceMicStatus> {
  return (await invoke<PresenceMicStatus>("presence_mic_stop")) as PresenceMicStatus;
}

export function presenceSetReducedMotion(enabled: boolean): Promise<void> {
  return safeInvoke("presence_set_reduced_motion", { enabled }).then(() => undefined);
}

/**
 * Session-only reduced-motion apply (Phase 4). Unlike
 * `presenceSetReducedMotion`, this does **not** persist to
 * `EdgeSettings` — used by the OS-preference watcher so the
 * accessibility setting layers over the runtime without stomping
 * an explicit user toggle stored on disk. Explicit user toggles
 * continue to go through `presenceSetReducedMotion` and take
 * precedence on next boot.
 */
export function presenceApplyReducedMotion(enabled: boolean): Promise<void> {
  return safeInvoke("presence_apply_reduced_motion", { enabled }).then(() => undefined);
}

export function presenceSetPalette(palette: PaletteId): Promise<void> {
  return safeInvoke("presence_set_palette", { palette }).then(() => undefined);
}

export function presenceSetRingWanted(wanted: boolean): Promise<void> {
  return safeInvoke("presence_set_ring_wanted", { wanted }).then(() => undefined);
}

export function presenceSetQualityTier(tier: QualityTier): Promise<void> {
  return safeInvoke("presence_set_quality_tier", { tier }).then(() => undefined);
}

/**
 * Toggles click-through on the presence droplet. `false` (default at
 * spawn) lets clicks fall through to windows behind the droplet;
 * `true` grabs mouse events so the user can drag or right-click it.
 * On a non-transparent build this is a no-op that logs on the runtime
 * side — safe to call unconditionally from the UI.
 */
export function presenceSetInteractive(interactive: boolean): Promise<void> {
  return safeInvoke("presence_set_interactive", { interactive }).then(() => undefined);
}

/**
 * The list of modes in the order the debug panel should render them.
 * Order matches `PresenceMode::ALL` in `presence-core::scene::mode` so
 * the two UIs (this one and the runtime's egui overlay) stay
 * comparable when both are open on the same machine.
 */
export const PRESENCE_MODES: { id: PresenceMode; label: string }[] = [
  { id: "thinking", label: "Thinking" },
  { id: "speaking", label: "Speaking" },
  { id: "tool_use", label: "Tool use" },
  { id: "listening", label: "Listening" },
  { id: "attention", label: "Attention" },
  { id: "error", label: "Error" },
];

/**
 * Fires a completion through the shell-embedded `AiRouter` (Phase 3
 * §3.2). `thinking` engages on entry, releases on outcome. Denied /
 * ApprovalRequired / Failed additionally fire the error pulse and
 * reject the promise with a human-readable message. `EchoBackend`
 * is the default backend today — the response is `"echo: <prompt>"`.
 */
export function assistantThink(prompt: string): Promise<string> {
  return invoke<string>("assistant_think", { prompt });
}

/**
 * One event on a streaming completion. Mirror of
 * `ralleh_ai_router::CompletionStreamEvent`. Terminal events
 * (`done` / `failed` / `denied` / `approval_required` /
 * `no_backend_configured`) are guaranteed to fire exactly once
 * per stream; the invocation promise resolves once the terminal
 * event has been dispatched to `onEvent`.
 */
export type CompletionStreamEvent =
  | { event: "chunk"; backend: string; text: string }
  | { event: "done"; backend: string }
  | { event: "failed"; backend: string; error: string }
  | { event: "denied" }
  | { event: "approval_required" }
  | { event: "no_backend_configured" };

/**
 * Streaming counterpart to `assistantThink`. Same policy, same
 * mode engagements — chunks arrive on `onEvent` in order, then a
 * single terminal event. See `assistant_think_stream` on the
 * Rust side. Uses Tauri's typed `Channel<T>` under the hood.
 */
export async function assistantThinkStream(
  prompt: string,
  onEvent: (event: CompletionStreamEvent) => void,
): Promise<void> {
  const channel = new Channel<CompletionStreamEvent>();
  channel.onmessage = onEvent;
  await invoke("assistant_think_stream", { prompt, onEvent: channel });
}

/**
 * Dispatches a scaffold call through the shell-embedded
 * `ToolGateway`. `tool_use` engages for the duration. Same
 * pass/fail policy as `assistantThink` — a Denied / Failed outcome
 * pulses `error` and rejects.
 */
export function assistantToolPing(): Promise<string> {
  return invoke<string>("assistant_tool_ping");
}

/**
 * Fire a sparse `attention` pulse (Phase 3 §3.4). Optional
 * `durationMs` overrides the ~450 ms default; the shell clamps to a
 * lower floor so anything below ~200 ms still produces a visible
 * pulse. Use for inbound notifications, one-shot "look here"
 * signals, and demoing the scan-sweep pattern from the dev panel.
 */
export function assistantNotifyInbound(durationMs?: number): Promise<void> {
  return invoke("assistant_notify_inbound", { durationMs });
}

export const PALETTES: { id: PaletteId; label: string }[] = [
  { id: "teal", label: "Teal" },
  { id: "lime", label: "Lime" },
  { id: "ice", label: "Ice" },
  { id: "ember", label: "Ember" },
];

// ---- Backend configuration surface --------------------------------
//
// Mirror of the Rust-side shapes in `settings.rs`. Keep the string
// discriminants in sync with `CompletionKind`'s serde rename policy
// (lowercase). Field names use camelCase to match `#[serde(rename_all
// = "camelCase")]` on the Rust side.

export type CompletionKind = "echo" | "openai" | "anthropic";

/** Write-only sentinel over the API key. `keep` is the safe default
 * so the frontend can never accidentally clear a stored key by
 * omitting the field. */
export type ApiKeyUpdate =
  | { op: "keep" }
  | { op: "clear" }
  | { op: "set"; value: string };

export type CompletionConfigUpdate = {
  kind: CompletionKind;
  baseUrl: string;
  model: string;
  apiKey: ApiKeyUpdate;
};

/** Where the API key actually lives. `keychain` = OS-native secure
 * store (Windows Credential Manager, macOS Keychain, Linux Secret
 * Service). `cleartext` = fallback to `edge-settings.json` because
 * no keychain was available on this host — the UI should render a
 * visible warning. `none` = nothing stored yet (distinct from an
 * insecure fallback so we don't scare-warn a first-time operator). */
export type SecretStorage = "keychain" | "cleartext" | "none";

/** Redacted view of what's persisted -- `hasApiKey: true` means a
 * key is stored, but the actual value never leaves the Rust side.
 * `storage` tells the UI which backing store the key lives in. */
export type RedactedCompletionConfig = {
  kind: CompletionKind;
  baseUrl: string;
  model: string;
  hasApiKey: boolean;
  storage: SecretStorage;
};

export type BackendStatus = {
  /** Name the router will attribute the next request to. Stable
   * strings: `local-echo`, `openai-compatible`, `anthropic`. */
  activeBackend: string;
  /** Persisted UI-owned config, redacted. `null` means the operator
   * has never opened the config UI; the shell is on env-var + Echo
   * defaults. */
  configured: RedactedCompletionConfig | null;
};

export type BackendTestResult = {
  ok: boolean;
  backend: string;
  latencyMs: number | null;
  sampleResponse: string | null;
  error: string | null;
};

/**
 * Snapshot the currently-active backend and the persisted UI config.
 * Called on settings-panel mount and after every save.
 */
export function assistantBackendStatus(): Promise<BackendStatus> {
  return invoke<BackendStatus>("assistant_backend_status");
}

/**
 * Run a real "hello"-tier completion against the proposed config
 * without touching the production router. Returns success, latency,
 * and a short response preview so the UI can render a proper
 * "connected: 320 ms" affordance.
 */
export function assistantTestBackend(
  config: CompletionConfigUpdate,
): Promise<BackendTestResult> {
  return invoke<BackendTestResult>("assistant_test_backend", { config });
}

/**
 * Persist the proposed config and hot-swap the router to use it.
 * Pass `null` to clear the persisted config -- the router falls
 * back to whatever `RALLEH_COMPLETION_*` env vars are set, and Echo
 * beyond that.
 */
export function assistantSaveBackend(
  config: CompletionConfigUpdate | null,
): Promise<BackendStatus> {
  return invoke<BackendStatus>("assistant_save_backend", { config });
}

/** One event line from the audit log. Mirrors the Rust
 * `AuditEvent` shape exactly. Field names stay stable across
 * versions -- external tooling (SIEM ingestion, jq filters) parses
 * these directly. */
export type AuditEvent = {
  timestamp: string;
  kind:
    | "egress-allow"
    | "egress-deny"
    | "backend-swap"
    | "secret-write"
    | "secret-clear"
    | "secret-migrate"
    | "secret-migrate-failed";
  outcome: "allow" | "deny";
  tenant: string;
  device: string;
  actor: string;
  subject: string;
  detail: unknown;
};

/** Read the last `limit` audit events. Bounded on the Rust side to
 * `[1, 500]` so a runaway UI can't stall the IPC bridge. */
export function assistantAuditTail(limit = 50): Promise<AuditEvent[]> {
  return invoke<AuditEvent[]>("assistant_audit_tail", { limit });
}

/** Tail the presence-runtime stderr capture (rotated
 * `presence.log` under the app config dir). Use to triage a
 * `presence-stalled` audit event — the last few hundred lines
 * usually contain the panic trace or driver warning that
 * preceded the stall. Bounded on the Rust side to `[1, 1000]`. */
export function presenceLogTail(limit = 100): Promise<string[]> {
  return invoke<string[]>("presence_log_tail", { limit });
}

/** UI helper: the list of supported providers with their labels
 * and a short human-readable hint. Kept next to the wire type so a
 * new provider is a two-file change (this + Rust). */
export const COMPLETION_KINDS: {
  id: CompletionKind;
  label: string;
  hint: string;
}[] = [
  {
    id: "echo",
    label: "Echo (local test)",
    hint: "No network. Response is `echo: <prompt>`. Safe default.",
  },
  {
    id: "openai",
    label: "OpenAI-compatible",
    hint: "OpenAI, Ollama, LM Studio, vLLM — anything speaking /chat/completions.",
  },
  {
    id: "anthropic",
    label: "Anthropic",
    hint: "Claude via the messages API. Requires an API key.",
  },
];
