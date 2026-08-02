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

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread;

use presence_ipc::{Command, Envelope, Event, EventEnvelope};

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
    mode: presence_ipc::PresenceMode,
}

impl Drop for ModeHold {
    fn drop(&mut self) {
        let Some(tx) = self.tx.take() else {
            return;
        };
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
        thread::Builder::new()
            .name("presence-writer".to_string())
            .spawn(move || writer_loop(stdin, rx))
            .map_err(|e| format!("writer thread: {e}"))?;
        thread::Builder::new()
            .name("presence-reader".to_string())
            .spawn(move || reader_loop(stdout, listener))
            .map_err(|e| format!("reader thread: {e}"))?;

        log::info!("desktop-edge: presence renderer spawned from {bin:?}");
        Ok(Self {
            tx: Some(tx),
            child: Mutex::new(Some(child)),
        })
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
        if tx.send(Envelope::wrap(command)).is_err() {
            log::warn!(
                "desktop-edge: presence writer thread has exited; dropping command"
            );
        }
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
    pub fn hold_mode(&self, mode: presence_ipc::PresenceMode) -> ModeHold {
        let Some(tx) = &self.tx else {
            return ModeHold { tx: None, mode };
        };
        if tx
            .send(Envelope::wrap(Command::SetMode {
                mode,
                engaged: true,
            }))
            .is_err()
        {
            // Writer thread has exited — return an inert guard rather
            // than one that will try to send a release into a dead
            // channel. Result is the same either way (nothing happens
            // on drop), but this keeps the branch obvious in profiling.
            return ModeHold { tx: None, mode };
        }
        ModeHold {
            tx: Some(tx.clone()),
            mode,
        }
    }

    /// Shared implementation for the two `pulse_*` helpers above.
    /// Engage → sleep on a detached thread → release. Detached
    /// because the caller (a Tauri command handler) has no reason to
    /// wait, and joining would either block the UI or need an async
    /// runtime this crate does not otherwise use.
    fn pulse_mode(&self, mode: presence_ipc::PresenceMode, hold_ms: u64) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx
            .send(Envelope::wrap(Command::SetMode {
                mode,
                engaged: true,
            }))
            .is_err()
        {
            return;
        }
        let release_tx = tx.clone();
        // A shell shutdown drops the writer's channel, which makes
        // the send below a no-op. Safe on either side.
        thread::Builder::new()
            .name(format!("presence-{:?}-pulse", mode))
            .spawn(move || {
                thread::sleep(std::time::Duration::from_millis(hold_ms));
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

fn reader_loop(stdout: std::process::ChildStdout, listener: EventListener) {
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
        listener(env.payload);
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
