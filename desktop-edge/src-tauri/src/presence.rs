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

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread;

use presence_ipc::{Command, Envelope};

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
    pub fn spawn_from_env() -> Self {
        let Some(bin) = std::env::var_os(BIN_ENV).map(PathBuf::from) else {
            log::info!(
                "desktop-edge: {BIN_ENV} unset — presence renderer disabled \
                 (set to the path of presence-runtime.exe to enable)"
            );
            return Self::disabled();
        };
        match Self::spawn(bin) {
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

    fn spawn(bin: PathBuf) -> Result<Self, String> {
        // `PRESENCE_STDIN_IPC=1` is the opt-in the runtime looks for
        // (see `presence-runtime/src/ipc_stdin.rs`). Without it the
        // runtime ignores stdin and the whole transport is inert, which
        // would leave us with a floating window we couldn't drive.
        let mut child = ProcessCommand::new(&bin)
            .env("PRESENCE_STDIN_IPC", "1")
            // The droplet chrome from Phase 2 §3 second slice. Skipping
            // this env var would give us the full 960x720 dev harness
            // with an egui panel — useful for local debugging but not
            // the shape the shell wants to embed.
            .env("PRESENCE_DROPLET", "1")
            .stdin(Stdio::piped())
            // Passing stdout/stderr through keeps the runtime's logs
            // visible in the same console the shell is launched from —
            // essential while the feature is opt-in via env var, because
            // any spawn issue has to surface *somewhere*.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn {bin:?}: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "presence-runtime spawn: no stdin handle".to_string())?;

        let (tx, rx) = mpsc::channel::<Envelope>();
        thread::Builder::new()
            .name("presence-writer".to_string())
            .spawn(move || writer_loop(stdin, rx))
            .map_err(|e| format!("writer thread: {e}"))?;

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
}
