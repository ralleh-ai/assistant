// Typed wrappers around the Tauri commands defined in
// `src-tauri/src/lib.rs` (`presence_*`). Every value in this file is
// mirroring a `presence_ipc` type from `crates/presence-ipc/src/lib.rs`
// — if a wire spelling changes there, it changes here too. Kept as
// literal-union types rather than TS enums so a stale string surfaces
// as a compile error at the callsite, not at runtime.
//
// Phase 2 §3 of docs/PRESENCE_INTEGRATION_PLAN.md. The corresponding
// receiver is `SceneDirector::apply_command` in `presence-core`.

import { invoke, InvokeArgs } from "@tauri-apps/api/core";

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
  return status ?? { enabled: false };
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

export const PALETTES: { id: PaletteId; label: string }[] = [
  { id: "teal", label: "Teal" },
  { id: "lime", label: "Lime" },
  { id: "ice", label: "Ice" },
  { id: "ember", label: "Ember" },
];
