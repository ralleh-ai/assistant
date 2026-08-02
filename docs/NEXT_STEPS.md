# Next Steps — Prioritized Backlog

## Done recently

- Product setup UX (splash → settings gate → core shell + voice style);
  live mic / OS caps / station settings foundations underneath.

## High priority — Tauri desktop shell (Phase 1 continued)

1. ~~Scaffold `desktop-edge/`~~ **done**
2. ~~Wire health / echo IPC (`core_ping`)~~ **done**
3. ~~Embed voice core (mock pipeline via `voice_smoke`)~~ **done**
4. ~~Settings / onboarding UI~~ **done** — product Settings gate + voice style;
   core placeholder with settings gear.
5. ~~OS capabilities (clipboard first)~~ **done** — traits + mocks; screen/hotkey
   stubs; `clipboard_smoke` via policy + mock (optional `--features clipboard-os`).
6. ~~Optional live mic from the shell~~ **done** — `mic_smoke` (policy + clearance);
   **on by default** in `desktop-edge` (`mic` feature / `build.features`).
   Workspace audio-core stays mic-off for headless CI.

## Medium priority — Point cloud presence entity (design locked, Phase 1 in progress)

See [`PRESENCE_VISUAL_ENTITY.md`](./PRESENCE_VISUAL_ENTITY.md),
[`PRESENCE_SCENES.md`](./PRESENCE_SCENES.md),
[`PRESENCE_INTEGRATION_PLAN.md`](./PRESENCE_INTEGRATION_PLAN.md),
[ADR-010](./adr/adr-010-point-cloud-presence-entity.md),
[ADR-011](./adr/adr-011-surface-point-generation-and-palette-setting.md),
[ADR-012](./adr/adr-012-additive-mode-composition.md), and
[ADR-013](./adr/adr-013-presence-window-and-process-model.md) for the full plan.

7. ~~Phase 1 — standalone Rust prototype~~ **substantially complete
   (2026-08-02)** — `presence-prototype/` (`winit` + `wgpu` + `noise`);
   Idle (Presence Shell) + Loading (Resonance Plate) clearly
   distinguishable, activity modes `thinking`/`speaking`/`tool_use` as
   weighted terms on the same shell (ADR-012), material-only modes
   `listening`/`attention`/`error` on the same layer without new
   geometry, multi-entity hierarchy via `activity_scale`, reduced-motion
   preset, quality tiers with adaptive downshift, `SceneRegistry`
   productized. 79 unit tests, clippy clean, ~170–200 FPS at idle on 2
   cores. Remaining Phase 1 items are opportunistic rather than
   blocking Phase 2:
   - Very-long-run (30+ min) peripheral idle QA — currently informal.
   - Optional `High` quality tier (100k+) if a target machine warrants it.
   - GPU compute path for deformation — deferred per ADR-011's fallback
     ordering; not needed at current numbers.

8. **Phase 2 — window productization and desktop-edge integration**
   (design locked in [ADR-013](./adr/adr-013-presence-window-and-process-model.md);
   implementation is the next major body of work). Concrete tasks:

   1. **Refactor `presence-prototype/` toward a shippable crate.** Split
      the current binary into `presence-core` (renderer + simulation,
      pure library — no `winit`, no key handling, no debug overlay) and
      `presence-runtime` (the binary that opens a window and runs the
      loop). The dev debug panel becomes a `dev` feature on the runtime
      so a production build does not ship it. See open item #2 in
      `PRESENCE_INTEGRATION_PLAN.md` §7 — promote, don't rewrite.
   2. **Define the IPC surface.** A `presence-ipc` crate holding the
      wire type for `PresenceSignals` (already the same shape the dev
      panel drives) plus a settings-update message (palette, quality
      tier, reduced-motion, window bounds). Transport and encoding are
      still open per ADR-013's "Not decided here" — protobuf-over-
      length-prefixed-frames is the leading candidate; JSON is easy but
      wasteful for a 60 Hz signal stream. Enforce the T9/T14 constraint
      in the *type*: no field capable of holding raw audio, transcript,
      or prompt/completion content.
   3. ~~**Frameless / transparent / always-on-top droplet (Windows
      first).**~~ **transparency + click-through landed
      (2026-08-02)** — `PRESENCE_TRANSPARENT=1` (auto-implies droplet
      chrome) asks winit for a transparent window, configures the
      swapchain for `CompositeAlphaMode::PreMultiplied`, and switches
      the composite shader to coverage-derived premultiplied alpha.
      `set_cursor_hittest(false)` makes clicks pass through by
      default. Focus grab lands via `Command::SetInteractive` and
      the dev panel's "Grab" toggle (2026-08-02); a global hotkey
      binding on the shell side is a UX follow-up but not blocking.
   4. ~~**Position and layout persistence.**~~ **landed
      (2026-08-02)** — reverse-channel `Event::Ready` / `Event::Moved`
      on the runtime's stdout (opt-in `PRESENCE_STDOUT_IPC=1`) plus
      `Command::SetPosition` on the way in. `desktop-edge` reads
      events on a reader thread, persists to
      `EdgeSettings.presence_position`, and echoes the value back on
      the next launch so the droplet lands where the user last left
      it. Multi-monitor placement is still open (deferred with
      ADR-013 §"Not decided here"); the current wire is
      single-screen physical pixels.
   5. **Launch and discovery.** How the shell finds / spawns the
      presence process. Options: shell-spawned child, user-launched
      alongside the shell, OS service. Not decided; prototype at least
      shell-spawned in this phase since it is the simplest and does
      not preclude the others.
   6. ~~**`EdgeSettings.presence_*` fields**~~ **landed
      (2026-08-02)** — `presence_palette`, `presence_quality_tier`,
      and `presence_reduced_motion` on `EdgeSettings`, all
      `#[serde(default)]` for backwards-compat. Every
      `presence_set_*` Tauri command now writes through to settings;
      `restore_presence_state` echoes them into the runtime right
      after spawn, so a user's chosen colour/tier/accessibility
      preset survives a restart.
   7. ~~**Still driven by synthetic signals**~~ **first real signal
      wired (2026-08-02)** — live mic → smoothed audio level →
      `Command::SetSignalsScalars` via the shell's `MicPump`. The pump
      is opt-in (dev-panel toggle, requires mic clearance) and the
      wire path now supports scalars-only updates that never touch
      mode engagement. `intensity` and `progress` are still synthetic;
      those move under Phase 3 alongside the VAD/router work.

9. **Phase 3 — real signals** (in progress):
    1. ~~Replace synthetic signals with the real VAD state machine.~~
       **VAD → `Listening` landed (2026-08-02)** — the mic pump now
       runs `ralleh-audio-core`'s `VoiceActivityDetector` alongside
       the RMS integrator and engages `PresenceMode::Listening`
       on the debounced `Speech` boundary. Releases the mode on
       both silence *and* pump stop, so a click-off cleans up.
       **Idle = "no work in flight" landed (2026-08-02).**
       `AssistantState` now holds an `Arc<AtomicUsize>` in-flight
       counter with a `WorkGuard` RAII handle; every command that
       engages a sustained mode (`assistant_think`,
       `assistant_tool_ping`) holds one alongside its `ModeHold`.
       `is_idle()` / `in_flight_handle()` expose the observation
       surface — consumed today by the §3.4 scan sweep, and by
       future status-line / telemetry observers.
    2. **Router + tool gateway embedded in the shell (2026-08-02).**
       `desktop-edge/src-tauri/src/assistant.rs` owns an
       `AiRouter` (default `EchoBackend`) and a `ToolGateway`
       (default `EchoHandler` registered under
       `assistant.tool.echo`). Two new Tauri commands —
       `assistant_think` (async, hits the router) and
       `assistant_tool_ping` (sync, hits the gateway) —
       hold `PresenceMode::Thinking` / `ToolUse` for the wall-clock
       duration of the call via `Presence::hold_mode` (RAII guard,
       drop-safe across `.await` / panics / `?` early returns).
       Denied / ApprovalRequired / Failed outcomes pulse `error` and
       reject the promise. Dev panel gains "Think" and "Tool call"
       chips. Real HTTP / LLM backends slot in behind
       `CompletionBackend` in Phase 4; the mode-signal path does not
       change.
    3. **`speaking` engagement + live `audio_level` pump
       (2026-08-02).** The Tauri `voice_smoke` handler fires
       `Presence::pulse_speaking(duration_ms)` on a successful
       synthesis and spawns `presence_speaking::spawn` on the
       same wall-clock. The pump chunks the TTS PCM at ~30 Hz,
       computes per-window RMS, EMA-smooths it, and pushes
       `Command::SetSignalsScalars` — the same wire path the mic
       pump uses. The mode engagement gives the shell "speaking";
       the scalar envelope gives it a syllable-following level.
       Pump handles empty / zero-rate inputs as no-ops and caps
       wall-clock at 30 s. When real cpal playback lands, the
       pump moves from `Vec<f32>` to a ringbuffer tap on the
       output stream with no change to the wire shape.
    4. **Sparse secondary events landed (2026-08-02).**
       `Presence::pulse_attention(duration_ms)` fires
       `PresenceMode::Attention` for a short hold (~450 ms default,
       clamped ≥200 ms). Two entry points: `assistant_notify_inbound`
       Tauri command (dev-panel "Notify" chip; ready for a real
       notification source), and an opt-in scan-sweep background
       thread gated by `RALLEH_SCAN_SWEEP_MS`. The sweep only fires
       when `AssistantState::is_idle()` returns true, so attention
       never layers on top of thinking / tool-use / speaking —
       preserving the "sparse" side of the anti-patterns list. A
       minimum 5 s interval is enforced regardless of the env var:
       a scan sweep is not a heartbeat.
    5. ~~Map policy `Denied` / handler `Failed` outcomes to
       `error`.~~ **landed (2026-08-02)** — the three Tauri smoke
       commands (`voice_smoke`, `clipboard_smoke`, `mic_smoke`) fire
       `Presence::pulse_error()` on every `Err(_)`. Detached ~600 ms
       pulse via a shared `pulse_mode` helper; no async runtime
       needed.

10. **Phase 4 — hardening and options**:
    - 60 FPS budget confirmation on representative hardware (Phase 1's
      2-core measurement is a floor, not a target-machine test).
    - OS-level reduced-motion preference honored automatically
      alongside the runtime toggle.
    - Optional text status line for accessibility (present-tense
      "listening", "thinking", etc.).
    - Rapid-state-change stress test with the real signal path.
    - Extension-point documentation for new entity types
      (`PRESENCE_SCENES.md` §8 already covers scenes; add entities).

## Medium priority

11. **OIDC / device attestation** — when NestJS control plane exists (T1/T18).
12. Optional `allow_private_targets` for http-fetch internal APIs.
13. Live mic → VAD → STT path in the shell (beyond capture metrics).
14. Approval cryptographically bound to approver identity (T4).
15. Audit integrity / queryability beyond JSONL (T5).
16. Real screen capture / hotkey OS backends (still trait-only stubs).

## Lower priority

- NestJS control plane, Postgres, Temporal — Phase 2+.
- MCP connectors — Phase 3.
- Native in-crate `piper-rs`.
- Mass-rename crates — deferred ([`CRATE_NAMING.md`](./CRATE_NAMING.md)).
