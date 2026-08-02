//! Stdin transport for [`presence_ipc::Envelope`] payloads.
//!
//! Opt-in via the `PRESENCE_STDIN_IPC=1` environment variable, because
//! stdin on a normal `cargo run` is a TTY and reading from it would eat
//! the user's keystrokes. When enabled, this module spawns one background
//! thread that reads newline-delimited JSON envelopes from stdin and
//! forwards their inner [`Command`]s over an [`mpsc::channel`] to the
//! main event loop, which drains the channel at the top of every
//! frame (see [`App::drain_pending_commands`]).
//!
//! # Wire format
//!
//! One envelope per line. Blank lines are ignored. A line that fails to
//! parse as JSON is logged at `warn` and dropped — the peer is expected
//! to log the same and continue rather than tear down the stream.
//!
//! # Failure modes
//!
//! - **Malformed JSON** — logged, dropped.
//! - **Stale [`Envelope::version`]** — logged, dropped. The peer should
//!   coordinate a version bump with this build before sending mismatched
//!   payloads; a receiver silently accepting them would let a shipping
//!   Envelope drift out from under its schema.
//! - **Unknown `Command` variant** — accepted here (the payload is a
//!   valid envelope from *this* build's perspective); the wildcard arm
//!   in [`SceneDirector::apply_command`] handles it further downstream.
//! - **EOF on stdin** — the thread exits. The main loop keeps running;
//!   the presence just stops receiving new commands.
//! - **`Sender` disconnected** — the thread exits. The main loop is
//!   gone, so continuing would be waste.
//!
//! # Why not `tokio` / `async`?
//!
//! Because stdin is one blocking line-oriented source, and adding a full
//! async runtime for a single reader thread would pay for something no
//! other part of `presence-runtime` needs.

use std::io::BufRead;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use presence_ipc::{Command, Envelope};

/// Environment variable that enables the stdin transport when set to a
/// truthy value (`1`, `true`, `yes`, case-insensitive). Any other value
/// (including unset) leaves the transport off.
pub const OPT_IN_ENV: &str = "PRESENCE_STDIN_IPC";

/// Returns a receiver iff [`OPT_IN_ENV`] is set to a truthy value. When
/// disabled — the default — this returns `None` and does not spawn a
/// thread, so behavior matches the pre-transport prototype exactly.
pub fn spawn_if_enabled() -> Option<Receiver<Command>> {
    if !opted_in() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("presence-ipc-stdin".to_string())
        .spawn(move || run(std::io::stdin().lock(), tx))
        .expect("failed to spawn stdin ipc thread");
    log::info!(
        "presence-runtime: {OPT_IN_ENV}=1 — stdin ipc thread started, \
         expecting newline-delimited presence-ipc envelopes"
    );
    Some(rx)
}

/// Drain everything the transport has produced since the last call and
/// return it as a `Vec<Command>` in arrival order. Returns an empty vec
/// when nothing is queued or the sender has disconnected.
///
/// Kept out of `App` so the drain logic is unit-testable without a
/// [`winit`] event loop.
pub fn drain(rx: &Receiver<Command>) -> Vec<Command> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(cmd) => out.push(cmd),
            // Empty is the common case — the shell may not have sent
            // anything since the previous frame. Disconnected is
            // terminal for this session but does not warrant a panic;
            // the presence just runs without new inputs from here on.
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return out,
        }
    }
}

fn opted_in() -> bool {
    matches!(
        std::env::var(OPT_IN_ENV)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Reads lines from `input` and forwards decoded commands to `tx`. Exits
/// on EOF, an I/O error, or a disconnected receiver.
///
/// `pub(crate)` for the tests below — external callers should go through
/// [`spawn_if_enabled`].
pub(crate) fn run<R: BufRead>(mut input: R, tx: Sender<Command>) {
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => return, // EOF
            Ok(_) => {}
            Err(err) => {
                log::warn!("presence-runtime: stdin ipc read error: {err}");
                return;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some(cmd) = decode(trimmed) else { continue };
        if tx.send(cmd).is_err() {
            return; // main loop gone
        }
    }
}

/// Parses one line into a `Command`, logging and returning `None` for
/// anything invalid. Pulled out of `run` so both the parse errors and
/// the version-mismatch path are covered by unit tests.
pub(crate) fn decode(line: &str) -> Option<Command> {
    match serde_json::from_str::<Envelope>(line) {
        Ok(envelope) if envelope.is_current() => Some(envelope.payload),
        Ok(envelope) => {
            log::warn!(
                "presence-runtime: dropping ipc envelope with unsupported \
                 version {} (this build expects {})",
                envelope.version,
                presence_ipc::VERSION
            );
            None
        }
        Err(err) => {
            log::warn!("presence-runtime: dropping malformed ipc envelope: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use presence_ipc::{PresenceMode, Signals};
    use std::io::Cursor;

    #[test]
    fn decode_accepts_a_current_envelope() {
        let env = Envelope::wrap(Command::SetReducedMotion { enabled: true });
        let line = serde_json::to_string(&env).unwrap();
        let out = decode(&line).expect("current envelope should decode");
        assert_eq!(out, Command::SetReducedMotion { enabled: true });
    }

    #[test]
    fn decode_drops_a_stale_version_without_panicking() {
        // Simulate a peer written against a future wire version. The
        // envelope decodes as a struct, but `is_current` returns false
        // and the transport must not surface it.
        let stale = Envelope {
            version: presence_ipc::VERSION.wrapping_add(1),
            payload: Command::SetReducedMotion { enabled: true },
        };
        let line = serde_json::to_string(&stale).unwrap();
        assert!(decode(&line).is_none());
    }

    #[test]
    fn decode_drops_malformed_json_without_panicking() {
        assert!(decode("{ this is not json").is_none());
        assert!(decode("\"a bare string\"").is_none());
    }

    #[test]
    fn run_forwards_every_line_in_order_and_stops_at_eof() {
        // Two well-formed envelopes with a blank line and a garbage
        // line mixed in — the transport must skip the noise, forward
        // the two commands in order, and exit cleanly on EOF.
        let a = Envelope::wrap(Command::SetSignals(Signals {
            intensity: 0.5,
            audio_level: 0.0,
            progress: 0.0,
            active_modes: vec![PresenceMode::Thinking],
        }));
        let b = Envelope::wrap(Command::SetRingWanted { wanted: true });
        let stream = format!(
            "{}\n\n{{ garbage }}\n{}\n",
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );

        let (tx, rx) = mpsc::channel();
        run(Cursor::new(stream), tx); // returns at EOF

        let out = drain(&rx);
        assert_eq!(out.len(), 2, "got {out:?}");
        assert!(matches!(out[0], Command::SetSignals(_)));
        assert!(matches!(out[1], Command::SetRingWanted { wanted: true }));
    }

    #[test]
    fn run_returns_when_the_receiver_disconnects() {
        // Drop the receiver so the first send fails; the thread must
        // exit rather than spin. If this test hangs, the disconnect
        // handling is broken.
        let env = Envelope::wrap(Command::SetReducedMotion { enabled: true });
        let stream = format!("{}\n", serde_json::to_string(&env).unwrap());

        let (tx, rx) = mpsc::channel();
        drop(rx);
        run(Cursor::new(stream), tx); // must not hang
    }
}
