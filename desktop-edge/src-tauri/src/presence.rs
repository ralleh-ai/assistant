//! Owns the `presence-runtime` child process and pushes
//! [`presence_ipc::Envelope`]s to it over stdin.
//!
//! Phase 2 §3 of `../../docs/PRESENCE_INTEGRATION_PLAN.md`. This is the
//! shell side of the transport whose receiver lives in
//! `presence-prototype/presence-runtime/src/ipc_stdin.rs`.
//!
//! # Discovery
//!
//! The path to the runtime binary is read from `RALLEH_PRESENCE_BIN` at
//! startup. When the variable is unset — the current default — presence
//! is *disabled*: [`Presence::send`] becomes a no-op and no child
//! process is spawned. That keeps the shell working on machines that
//! haven't built `presence-runtime` yet and lets the whole feature ship
//! dark while the visual side is iterated on.
//!
//! Once Phase 4 lands, the shell will bundle the runtime binary and this
//! env-var opt-in goes away.
//!
//! # Lifecycle
//!
//! - [`Presence::spawn_from_env`] is called once in `run()` and installed
//!   as Tauri managed state.
//! - Commands issued from the JS side (or from any Rust code holding a
//!   `State<'_, Presence>`) go through [`Presence::send`], which drops
//!   an [`Envelope`] onto an `mpsc::channel`.
//! - A dedicated writer thread pulls from the channel, serializes each
//!   envelope as one line of NDJSON, and writes it to the child's
//!   stdin. Kept off the Tauri command threads so a slow child can
//!   never back-pressure the UI.
//! - On [`Presence::drop`] the sender is dropped (writer thread sees
//!   EOF and exits), the child's stdin closes (its reader thread sees
//!   EOF), and the child is killed if it hasn't already exited.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use presence_ipc::{Command, Envelope, Event, EventEnvelope, PresenceMode};

/// Shared, thread-safe set of currently-engaged modes. Every code
/// path that engages or releases a mode on the wire also updates
/// this set, so the shell has a truthful answer to
/// "what's on air right now?" without needing a reverse-channel
/// event from the runtime. Consumed by the aria-live status line
/// (Phase 4 accessibility) and by future observers (telemetry).
///
/// `Arc<Mutex<_>>` (not `RwLock`) because the write side fires far
/// more often than the read side (every mode change vs. one poll
/// every 200 ms), and the critical sections are two-line
/// `HashSet::insert`/`remove` — a spinny `RwLock` would cost more
/// than it saves.
type ModeSet = Arc<Mutex<HashSet<PresenceMode>>>;

/// Liveness snapshot for the presence runtime. Shared between the
/// stdout reader (which stamps `last_event_at` on every incoming
/// envelope, heartbeat or otherwise) and the monitor thread (which
/// polls it against [`presence_ipc::STALL_THRESHOLD_MS`]). Also
/// carries the most recently seen heartbeat sequence + uptime so a
/// stall event has real telemetry attached rather than a bare
/// timestamp.
///
/// Wrapped in `Arc<Mutex<_>>` because every writer / reader is on a
/// distinct thread and the critical section is trivial (a handful of
/// field writes). A dedicated struct rather than four independent
/// atomics because the fields must stay consistent as a set —
/// reporting a fresh timestamp with a stale sequence number would
/// mislead the audit log.
#[derive(Debug, Clone)]
pub struct LivenessSnapshot {
    /// Wall-clock of the last event we successfully parsed off the
    /// child's stdout. `None` before any event arrives (including
    /// the initial `Event::Ready`) — the monitor treats that state
    /// as "still starting up" and does not flag it as a stall.
    pub last_event_at: Option<Instant>,
    /// Highest `sequence` field ever observed on an
    /// [`Event::Heartbeat`]. `None` before the first heartbeat.
    /// A regression (new sequence < old sequence) is logged and
    /// noted in the audit trail — it implies the runtime restarted
    /// inside the same process handle, which today should not
    /// happen but is worth catching if a future recovery path
    /// enables it.
    pub last_heartbeat_sequence: Option<u64>,
    /// Latest `uptime_ms` seen on a heartbeat. Persisted alongside
    /// stall events for postmortem correlation.
    pub last_heartbeat_uptime_ms: Option<u64>,
    /// Wall-clock of the process spawn. Combined with
    /// `SPAWN_GRACE` in the monitor to suppress "stall" false
    /// positives during window creation / first-frame warmup.
    pub spawned_at: Option<Instant>,
}

impl LivenessSnapshot {
    pub const fn new() -> Self {
        Self {
            last_event_at: None,
            last_heartbeat_sequence: None,
            last_heartbeat_uptime_ms: None,
            spawned_at: None,
        }
    }
}

impl Default for LivenessSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle held both by the reader thread (writes) and the monitor
/// thread (reads). Publicly typed so [`crate::assistant`] and the
/// audit-event helpers can consume snapshots without depending on
/// the private Presence fields.
pub type Liveness = Arc<Mutex<LivenessSnapshot>>;

/// Grace window after spawn during which "no events yet" is not a
/// stall. Covers cold-start on slow hardware (GPU driver init,
/// shader compile, wgpu adapter probing) — 15 s is a comfortable
/// upper bound; even the worst-case Linux Mesa cold path is well
/// under that in our stress tests.
pub const SPAWN_GRACE_MS: u64 = 15_000;

/// Health state machine reported by the monitor. Kept small and
/// stringly-typed at the audit layer, but here it is a plain enum
/// so the monitor's edge-detection can pattern match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceHealth {
    /// Runtime is disabled (no binary configured) — the monitor
    /// exits immediately.
    Disabled,
    /// Inside `SPAWN_GRACE_MS`, still awaiting first event.
    Starting,
    /// Last event within `STALL_THRESHOLD_MS`.
    Healthy,
    /// No event for `STALL_THRESHOLD_MS` or more.
    Stalled,
}

/// Callback the shell installs to react to reverse-channel [`Event`]s.
/// Boxed rather than a specific type so `Presence` doesn't drag every
/// consumer (Tauri, settings, logging) into its own signature.
pub type EventListener = Box<dyn Fn(Event) + Send + 'static>;

/// RAII guard returned by [`Presence::hold_mode`]. Engages a
/// [`presence_ipc::PresenceMode`] on construction (via the caller)
/// and releases it on drop. `Send` because `Sender<Envelope>` is,
/// which is what makes it safe to hold across `.await` points inside
/// Tauri async command handlers.
pub struct ModeHold {
    tx: Option<Sender<Envelope>>,
    mode: PresenceMode,
    /// Cloned handle to `Presence::engaged_modes` so `Drop` can
    /// update the shell-side tracker in the same critical path
    /// that sends the release envelope. `None` on an inert guard
    /// (disabled presence or dead writer) — mirrors `tx`.
    tracker: Option<ModeSet>,
}

impl Drop for ModeHold {
    fn drop(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
        if let Some(tracker) = self.tracker.take() {
            if let Ok(mut set) = tracker.lock() {
                set.remove(&self.mode);
            }
        }
        // Best-effort release. If the writer thread has since exited,
        // the send fails silently — the runtime will lose the mode
        // engagement on its next crossfade tick, which is the same
        // failure mode every other command has on a dead pipe.
        let _ = tx.send(Envelope::wrap(Command::SetMode {
            mode: self.mode,
            engaged: false,
        }));
    }
}

/// Environment variable that points at the `presence-runtime` binary.
/// Unset means "presence disabled" — see the module docs.
pub const BIN_ENV: &str = "RALLEH_PRESENCE_BIN";

/// Handle installed as Tauri managed state. `send` from anywhere; the
/// writer thread handles serialization and pipe I/O.
pub struct Presence {
    /// `None` when presence is disabled (the env var was unset or the
    /// spawn failed). `send` short-circuits to a no-op in that case.
    tx: Option<Sender<Envelope>>,
    /// Held so the process is killed on drop rather than orphaned.
    /// `Mutex` for interior mutability — `Presence` lives inside
    /// Tauri's `State`, which hands out shared references.
    child: Mutex<Option<Child>>,
    /// Shell-side truth for currently-engaged modes. Populated by
    /// every path that flips a mode on the wire (`send(SetMode)`,
    /// `hold_mode` + `ModeHold::drop`, `pulse_mode` engage +
    /// delayed release). Read by [`Presence::current_modes`] for
    /// the aria-live status line.
    engaged_modes: ModeSet,
    /// Reverse-channel liveness. Reader thread stamps this on
    /// every event; the stall monitor and the debug UI both read
    /// snapshots from it. Cloned into `Arc` so the monitor thread
    /// can outlive a `&Presence` reference without a lifetime
    /// scar in the Tauri command surface.
    liveness: Liveness,
}

impl Presence {
    /// Reads [`BIN_ENV`] and spawns the runtime if a path is set. Never
    /// returns an error — if the spawn fails we log and continue with a
    /// disabled `Presence`, because a missing renderer must never block
    /// the shell from starting.
    ///
    /// `listener` is invoked once per reverse-channel [`Event`]. It runs
    /// on the reader thread — do not block or acquire long-lived locks
    /// inside it. In this build the listener persists window geometry
    /// to `EdgeSettings`, which is a small `fs::write` and safe here.
    pub fn spawn_from_env(listener: EventListener) -> Self {
        let Some(bin) = std::env::var_os(BIN_ENV).map(PathBuf::from) else {
            log::info!(
                "desktop-edge: {BIN_ENV} unset — presence renderer disabled \
                 (set to the path of presence-runtime.exe to enable)"
            );
            return Self::disabled();
        };
        match Self::spawn(bin, listener) {
            Ok(p) => p,
            Err(err) => {
                log::warn!(
                    "desktop-edge: failed to spawn presence renderer — continuing \
                     without it: {err}"
                );
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            tx: None,
            child: Mutex::new(None),
            engaged_modes: Arc::new(Mutex::new(HashSet::new())),
            liveness: Arc::new(Mutex::new(LivenessSnapshot::new())),
        }
    }

    fn spawn(bin: PathBuf, listener: EventListener) -> Result<Self, String> {
        // `PRESENCE_STDIN_IPC=1` and `PRESENCE_STDOUT_IPC=1` opt the
        // runtime into the forward and reverse transports respectively
        // (see the ipc_stdin / ipc_stdout modules in presence-runtime).
        // Without stdout ipc the child would keep writing logs to the
        // parent terminal, which is fine for a dev harness but useless
        // for the shell that wants to parse `Event` NDJSON.
        let mut child = ProcessCommand::new(&bin)
            .env("PRESENCE_STDIN_IPC", "1")
            .env("PRESENCE_STDOUT_IPC", "1")
            // The droplet chrome from Phase 2 §3. Skipping this env var
            // would give us the full 960x720 dev harness with an egui
            // panel — useful for local debugging but not the shape the
            // shell wants to embed.
            .env("PRESENCE_DROPLET", "1")
            // Per-pixel alpha + click-through. Implies droplet on the
            // runtime side; setting both here is explicit and
            // future-proofs against the two flags diverging.
            .env("PRESENCE_TRANSPARENT", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr still inherited: `log::info!` from the runtime
            // (env_logger) writes there by default, and mixing it into
            // stdout would garble the NDJSON we now parse from stdout.
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {bin:?}: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "presence-runtime spawn: no stdin handle".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "presence-runtime spawn: no stdout handle".to_string())?;

        let (tx, rx) = mpsc::channel::<Envelope>();
        let liveness: Liveness = Arc::new(Mutex::new(LivenessSnapshot {
            last_event_at: None,
            last_heartbeat_sequence: None,
            last_heartbeat_uptime_ms: None,
            spawned_at: Some(Instant::now()),
        }));
        thread::Builder::new()
            .name("presence-writer".to_string())
            .spawn(move || writer_loop(stdin, rx))
            .map_err(|e| format!("writer thread: {e}"))?;
        let reader_liveness = liveness.clone();
        thread::Builder::new()
            .name("presence-reader".to_string())
            .spawn(move || reader_loop(stdout, listener, reader_liveness))
            .map_err(|e| format!("reader thread: {e}"))?;

        log::info!("desktop-edge: presence renderer spawned from {bin:?}");
        Ok(Self {
            tx: Some(tx),
            child: Mutex::new(Some(child)),
            engaged_modes: Arc::new(Mutex::new(HashSet::new())),
            liveness,
        })
    }

    /// Handle to the runtime's liveness snapshot. Cheap `Arc` clone,
    /// safe to hand to a background monitor thread. Returns a live
    /// (but empty) snapshot even for a disabled Presence so callers
    /// don't need a branch — the monitor sees "no spawn timestamp"
    /// and exits on its own.
    pub fn liveness_handle(&self) -> Liveness {
        self.liveness.clone()
    }

    /// One-shot snapshot for the debug UI / status commands.
    /// Cheap: locks the mutex, clones a small `Copy`-ish struct,
    /// releases.
    pub fn liveness_snapshot(&self) -> LivenessSnapshot {
        self.liveness
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    /// Fire-and-forget send. Never blocks the caller: enqueues the
    /// envelope on the writer thread's channel and returns immediately.
    /// A disabled `Presence` or a dead writer thread both silently drop
    /// the command — this is a *visual* signal, and dropping one is
    /// preferable to blocking a Tauri command handler.
    pub fn send(&self, command: Command) {
        let Some(tx) = &self.tx else {
            return;
        };
        // Update the shell-side tracker *before* the send so a poll
        // that lands in between here and the child ACK still returns
        // an accurate on-air set. The runtime treats a duplicate
        // engage/release as idempotent, so the local set can lead
        // the wire by a few microseconds without visual consequence.
        if let Command::SetMode { mode, engaged } = &command {
            self.record_mode(*mode, *engaged);
        }
        if tx.send(Envelope::wrap(command)).is_err() {
            log::warn!(
                "desktop-edge: presence writer thread has exited; dropping command"
            );
        }
    }

    /// Snapshot of currently-engaged modes. Sorted by label for
    /// deterministic UI order — the aria-live status line reads the
    /// first element in a specific priority order (see
    /// `PresenceStatusLine.tsx`), and shuffling the set on every
    /// poll would produce spurious re-announcements.
    pub fn current_modes(&self) -> Vec<PresenceMode> {
        let set = self.engaged_modes.lock();
        let Ok(set) = set else {
            return Vec::new();
        };
        let mut out: Vec<PresenceMode> = set.iter().copied().collect();
        out.sort_by_key(|m| m.label());
        out
    }

    fn record_mode(&self, mode: PresenceMode, engaged: bool) {
        if let Ok(mut set) = self.engaged_modes.lock() {
            if engaged {
                set.insert(mode);
            } else {
                set.remove(&mode);
            }
        }
    }

    /// Shared handle to the tracker for background paths (the pulse
    /// release thread most notably) that need to update it without
    /// holding a `&Presence` across a `.sleep`.
    fn tracker_clone(&self) -> ModeSet {
        self.engaged_modes.clone()
    }

    /// True iff the child was successfully spawned and the writer
    /// thread is (as far as we can tell) still accepting commands.
    /// Used by tests and by the debug `presence_status` command.
    pub fn is_enabled(&self) -> bool {
        self.tx.is_some()
    }

    /// Fires a short `PresenceMode::Error` engagement and releases it
    /// after the pulse hold. Matches the "brief error pulse,
    /// self-clearing" signature in `PRESENCE_VISUAL_ENTITY.md` — the
    /// runtime's own transition ramps handle the fade in and out; the
    /// hold below sets the total on-air time.
    ///
    /// The release is scheduled on a small detached thread rather
    /// than a `tokio::time::sleep`, because `Presence` predates any
    /// async runtime in this crate and adding one just for a 600 ms
    /// sleep is more surface area than a `thread::sleep` deserves.
    /// The thread exits as soon as the release lands.
    ///
    /// No-op on a disabled `Presence` — the caller can invoke this
    /// on every policy denial without checking `is_enabled` first.
    pub fn pulse_error(&self) {
        // Kept in one place so the hold matches everywhere it fires
        // (three Tauri command handlers today, more later). ~600 ms
        // sits inside the runtime's 300–900 ms transition window
        // and reads as one deliberate flash rather than a stutter.
        const ERROR_HOLD_MS: u64 = 600;
        self.pulse_mode(presence_ipc::PresenceMode::Error, ERROR_HOLD_MS);
    }

    /// Fires `PresenceMode::Speaking` for `duration_ms` then releases.
    /// The TTS path calls this with the wall-clock length of the
    /// synthesized utterance, so the visual holds for as long as the
    /// assistant would be talking. Once real playback is wired
    /// through cpal, this can gain a companion pump that also drives
    /// `audio_level` from chunked RMS of the outgoing samples — see
    /// Phase 3 §3.3.
    ///
    /// No-op on a disabled `Presence`. Duration is clamped to a
    /// small floor so a caller passing `0` still produces a visible
    /// pulse (the runtime's own attack/release below ~200 ms is
    /// visually indistinguishable from noise).
    pub fn pulse_speaking(&self, duration_ms: u64) {
        let hold = duration_ms.max(200);
        self.pulse_mode(presence_ipc::PresenceMode::Speaking, hold);
    }

    /// Fires `PresenceMode::Attention` for `duration_ms` then
    /// releases. Sparse events (`§3.4` — scan sweeps, inbound
    /// streams, notifications) hit this rather than a sustained
    /// mode: a bright pulse that settles back to whatever the shell
    /// was doing before is exactly the "look here, briefly" signal
    /// the anti-patterns list calls for.
    ///
    /// Same clamp as `pulse_speaking` — below ~200 ms the attack
    /// and release overlap into a blur rather than a distinct
    /// glance. Callers passing longer holds should be aware that
    /// attention layers over any concurrent mode, so a long hold on
    /// top of `speaking` reads as loud rather than as one event.
    pub fn pulse_attention(&self, duration_ms: u64) {
        let hold = duration_ms.max(200);
        self.pulse_mode(presence_ipc::PresenceMode::Attention, hold);
    }

    /// Engages `mode` and returns a guard that releases it on drop.
    /// The sustained counterpart to `pulse_*` — used by the router
    /// and tool-gateway wrappers where the visual must hold for the
    /// full duration of an async operation (which could be
    /// milliseconds for `EchoBackend` or many seconds for a real
    /// LLM) rather than a fixed hold.
    ///
    /// RAII drop is what makes this safe across `.await` points and
    /// early returns: even if the future panics or bails out with
    /// `?`, the release fires. Re-engaging the same mode from a
    /// nested call is idempotent on the runtime side, so nested
    /// guards behave sensibly as long as their `Drop` order is right
    /// — which Rust guarantees for scope-owned values.
    ///
    /// No-op on a disabled `Presence`: the returned guard's `Drop`
    /// is inert. Callers do not need to check `is_enabled` first.
    pub fn hold_mode(&self, mode: PresenceMode) -> ModeHold {
        let Some(tx) = &self.tx else {
            return ModeHold {
                tx: None,
                mode,
                tracker: None,
            };
        };
        self.record_mode(mode, true);
        if tx
            .send(Envelope::wrap(Command::SetMode {
                mode,
                engaged: true,
            }))
            .is_err()
        {
            // Writer thread has exited — roll back the tracker so we
            // don't advertise a mode that's not actually on air, and
            // return an inert guard. Result on drop is the same
            // either way (nothing happens), but the tracker rollback
            // matters for the status-line poll.
            self.record_mode(mode, false);
            return ModeHold {
                tx: None,
                mode,
                tracker: None,
            };
        }
        ModeHold {
            tx: Some(tx.clone()),
            mode,
            tracker: Some(self.tracker_clone()),
        }
    }

    /// Shared implementation for the two `pulse_*` helpers above.
    /// Engage → sleep on a detached thread → release. Detached
    /// because the caller (a Tauri command handler) has no reason to
    /// wait, and joining would either block the UI or need an async
    /// runtime this crate does not otherwise use.
    fn pulse_mode(&self, mode: PresenceMode, hold_ms: u64) {
        let Some(tx) = &self.tx else {
            return;
        };
        self.record_mode(mode, true);
        if tx
            .send(Envelope::wrap(Command::SetMode {
                mode,
                engaged: true,
            }))
            .is_err()
        {
            self.record_mode(mode, false);
            return;
        }
        let release_tx = tx.clone();
        let tracker = self.tracker_clone();
        // A shell shutdown drops the writer's channel, which makes
        // the send below a no-op. Safe on either side.
        thread::Builder::new()
            .name(format!("presence-{:?}-pulse", mode))
            .spawn(move || {
                thread::sleep(std::time::Duration::from_millis(hold_ms));
                if let Ok(mut set) = tracker.lock() {
                    set.remove(&mode);
                }
                let _ = release_tx.send(Envelope::wrap(Command::SetMode {
                    mode,
                    engaged: false,
                }));
            })
            .ok();
    }

    /// A clone of the envelope sender, for background pumps that need
    /// to push commands without going through a `send()` call per
    /// packet. `None` when presence is disabled — the caller must
    /// treat the absence as "no pump, no error". `Sender<Envelope>` is
    /// `Clone` so multiple pumps can hold their own copy without
    /// contention on a mutex.
    pub fn sender_clone(&self) -> Option<Sender<Envelope>> {
        self.tx.clone()
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        // Dropping the sender closes the channel; the writer thread's
        // `recv` returns `Err` and it exits, which drops `stdin`. The
        // child's reader thread sees EOF and exits too.
        self.tx = None;

        // Belt and braces: on a fast shutdown the child might not have
        // noticed EOF before the OS is about to kill the shell anyway,
        // and an orphaned always-on-top window would be genuinely
        // disruptive. Kill explicitly.
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn reader_loop(
    stdout: std::process::ChildStdout,
    listener: EventListener,
    liveness: Liveness,
) {
    // Line-buffered read of NDJSON `EventEnvelope` payloads. Malformed
    // lines are logged and skipped (same policy the forward path
    // uses); an EOF on stdout is the normal terminate signal, either
    // from the child exiting or from us tearing it down on drop.
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(err) => {
                log::warn!(
                    "desktop-edge: presence stdout read error ({err}); reader thread exiting"
                );
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let env: EventEnvelope = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(err) => {
                log::warn!(
                    "desktop-edge: dropping malformed presence event envelope: {err}"
                );
                continue;
            }
        };
        if !env.is_current() {
            log::warn!(
                "desktop-edge: dropping presence event with unsupported version {} \
                 (this build expects {})",
                env.version,
                presence_ipc::VERSION
            );
            continue;
        }
        // Stamp liveness *before* dispatching — the listener may
        // block briefly (writes to `EdgeSettings`), and we want the
        // "when did we last hear from the runtime?" clock to include
        // arrival, not completion of the shell's downstream work.
        if let Ok(mut snap) = liveness.lock() {
            snap.last_event_at = Some(Instant::now());
            if let Event::Heartbeat { sequence, uptime_ms } = &env.payload {
                if let Some(prev) = snap.last_heartbeat_sequence {
                    if *sequence < prev {
                        log::warn!(
                            "desktop-edge: presence heartbeat sequence regressed \
                             ({prev} -> {sequence}) — runtime may have restarted internally"
                        );
                    }
                }
                snap.last_heartbeat_sequence = Some(*sequence);
                snap.last_heartbeat_uptime_ms = Some(*uptime_ms);
            }
        }
        // Heartbeats are consumed by the liveness tracker only —
        // no downstream listener cares about them, so keep the
        // callback surface quiet rather than making every consumer
        // add a "drop heartbeat" arm.
        if !matches!(env.payload, Event::Heartbeat { .. }) {
            listener(env.payload);
        }
    }
}

fn writer_loop(mut stdin: std::process::ChildStdin, rx: Receiver<Envelope>) {
    while let Ok(env) = rx.recv() {
        // One envelope per line. Errors here are terminal — either the
        // pipe is broken (child exited) or the OS is under so much
        // pressure that we can't write bytes, and in both cases the
        // right thing is to stop trying rather than spin.
        let line = match serde_json::to_string(&env) {
            Ok(s) => s,
            Err(err) => {
                log::warn!("desktop-edge: presence envelope encode failed: {err}");
                continue;
            }
        };
        if let Err(err) = writeln!(stdin, "{line}") {
            log::warn!(
                "desktop-edge: presence stdin write failed ({err}); writer thread exiting"
            );
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_presence_swallows_sends_and_reports_disabled() {
        // The "no env var" path must never panic and must never block.
        // This is the state the shell is in on any machine that hasn't
        // built the runtime, and startup has to succeed there.
        let p = Presence::disabled();
        assert!(!p.is_enabled());
        p.send(Command::SetReducedMotion { enabled: true });
        p.send(Command::SetPalette {
            palette: presence_ipc::PaletteId::Ember,
        });
        // If any of the above blocked or panicked, this line does not
        // execute — the test failure would be a timeout or a stack
        // trace rather than an assertion.
    }

    #[test]
    fn hold_mode_engages_on_construction_and_releases_on_drop() {
        // Wire a Presence to an in-memory receiver so we can inspect
        // exactly which envelopes the guard emits. `Sender<Envelope>`
        // does not need the child process for this to work.
        let (tx, rx) = mpsc::channel::<Envelope>();
        let p = Presence {
            tx: Some(tx),
            child: Mutex::new(None),
            engaged_modes: Arc::new(Mutex::new(HashSet::new())),
            liveness: Arc::new(Mutex::new(LivenessSnapshot::new())),
        };

        {
            let _hold = p.hold_mode(presence_ipc::PresenceMode::Thinking);
            let engage = rx.recv().expect("engage envelope");
            assert!(matches!(
                engage.payload,
                Command::SetMode {
                    mode: presence_ipc::PresenceMode::Thinking,
                    engaged: true,
                }
            ));
        }

        // Guard dropped at end of block — release must be next on the
        // wire, with the same mode and `engaged: false`.
        let release = rx.recv().expect("release envelope");
        assert!(matches!(
            release.payload,
            Command::SetMode {
                mode: presence_ipc::PresenceMode::Thinking,
                engaged: false,
            }
        ));
    }

    #[test]
    fn current_modes_reflects_hold_and_release() {
        let (tx, _rx) = mpsc::channel::<Envelope>();
        let p = Presence {
            tx: Some(tx),
            child: Mutex::new(None),
            engaged_modes: Arc::new(Mutex::new(HashSet::new())),
            liveness: Arc::new(Mutex::new(LivenessSnapshot::new())),
        };
        assert!(p.current_modes().is_empty());
        {
            let _thinking = p.hold_mode(PresenceMode::Thinking);
            let _tool = p.hold_mode(PresenceMode::ToolUse);
            let modes = p.current_modes();
            assert_eq!(modes.len(), 2, "expected 2 engaged, got {modes:?}");
            assert!(modes.contains(&PresenceMode::Thinking));
            assert!(modes.contains(&PresenceMode::ToolUse));
        }
        assert!(
            p.current_modes().is_empty(),
            "modes must clear after guards drop"
        );
    }

    #[test]
    fn hold_mode_on_disabled_presence_is_inert() {
        // The Tauri command handlers use `hold_mode` unconditionally.
        // Constructing and dropping the guard against a disabled
        // `Presence` must be a no-op — no panic, no thread spawn, no
        // send-to-nowhere.
        let p = Presence::disabled();
        let hold = p.hold_mode(presence_ipc::PresenceMode::Thinking);
        drop(hold);
    }

    #[test]
    fn rapid_mode_flips_stay_balanced_and_never_leak_tracker_state() {
        // Phase 4 stress test. The failure mode this guards against
        // is asymmetric bookkeeping — an engage that skips the
        // tracker update, or a drop path that doesn't decrement —
        // which would leave `current_modes()` reporting a stuck
        // engagement after all real work has ended. 5,000 iterations
        // is well past the settle time of any realistic burst
        // (voice_smoke fires ~10 SetMode messages per pulse, so
        // this is ~500 pulses of load compressed into microseconds).
        let (tx, rx) = mpsc::channel::<Envelope>();
        let p = Presence {
            tx: Some(tx),
            child: Mutex::new(None),
            engaged_modes: Arc::new(Mutex::new(HashSet::new())),
            liveness: Arc::new(Mutex::new(LivenessSnapshot::new())),
        };

        for i in 0..5_000_u32 {
            // Alternate between three sustained modes so the set
            // grows and shrinks rather than trivially toggling one.
            let mode = match i % 3 {
                0 => PresenceMode::Thinking,
                1 => PresenceMode::ToolUse,
                _ => PresenceMode::Listening,
            };
            let hold = p.hold_mode(mode);
            drop(hold);
        }

        // Every guard was scope-owned above, so on entry to this
        // assertion the tracker MUST be empty regardless of channel
        // health. If it isn't, the engage/release paths have drifted
        // out of sync somewhere.
        assert!(
            p.current_modes().is_empty(),
            "tracker leaked: {:?} still engaged after 5000 balanced flips",
            p.current_modes()
        );

        // Also drain the channel and confirm we see exactly
        // 5000 engages + 5000 releases — no dropped sends and no
        // duplicates. This is the wire-level counterpart to the
        // tracker check above.
        let mut engages = 0_u32;
        let mut releases = 0_u32;
        while let Ok(env) = rx.try_recv() {
            match env.payload {
                Command::SetMode { engaged: true, .. } => engages += 1,
                Command::SetMode { engaged: false, .. } => releases += 1,
                other => panic!("unexpected envelope in stress stream: {other:?}"),
            }
        }
        assert_eq!(engages, 5_000, "engage count mismatch");
        assert_eq!(releases, 5_000, "release count mismatch");
    }

    #[test]
    fn pulse_helpers_are_safe_on_a_disabled_presence() {
        // The Tauri command handlers call these on every failure /
        // TTS run without checking `is_enabled` first. A machine that
        // hasn't set `RALLEH_PRESENCE_BIN` must not panic, must not
        // spawn a thread, and must not block.
        let p = Presence::disabled();
        p.pulse_error();
        p.pulse_speaking(500);
        // The line-reaching-this-point is the assertion.
    }

    #[test]
    fn reader_loop_forwards_valid_envelopes_and_skips_garbage() {
        use std::io::Cursor;
        // Build a fake `ChildStdout` — we can't construct one directly,
        // so reuse the loop's internals via a small helper. `reader_loop`
        // takes `ChildStdout` for real spawn ergonomics; here we test
        // the parsing rules against a `BufReader<Cursor>` instead.
        fn drive<R: BufRead>(reader: R, listener: EventListener) {
            for line in reader.lines() {
                let line = line.unwrap();
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(env) = serde_json::from_str::<EventEnvelope>(&line) else {
                    continue;
                };
                if !env.is_current() {
                    continue;
                }
                listener(env.payload);
            }
        }
        let a = EventEnvelope::wrap(Event::Ready { x: 10, y: 20 });
        let b = EventEnvelope::wrap(Event::Moved { x: 30, y: 40 });
        let stream = format!(
            "{}\n\n{{ garbage }}\n{}\n",
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );

        let received: std::sync::Arc<Mutex<Vec<Event>>> =
            std::sync::Arc::new(Mutex::new(Vec::new()));
        let received_cb = received.clone();
        drive(
            Cursor::new(stream),
            Box::new(move |e| received_cb.lock().unwrap().push(e)),
        );

        let got = received.lock().unwrap();
        assert_eq!(got.len(), 2, "got {got:?}");
        assert_eq!(got[0], Event::Ready { x: 10, y: 20 });
        assert_eq!(got[1], Event::Moved { x: 30, y: 40 });
    }
}
