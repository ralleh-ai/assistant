//! Reverse-channel transport (`presence-runtime` → shell).
//!
//! Symmetric with [`crate::ipc_stdin`]: the shell writes
//! [`presence_ipc::Envelope`]s to the child's stdin, the child writes
//! [`presence_ipc::EventEnvelope`]s to its stdout. One event per line
//! (NDJSON), same wire framing as the forward direction.
//!
//! # Opt-in
//!
//! Off by default — the dev harness leaves stdout as the terminal so
//! `println!` from `log::info!` remains readable. `PRESENCE_STDOUT_IPC=1`
//! flips the runtime into event mode; the shell sets that automatically
//! when it spawns the child (see `desktop-edge/src-tauri/src/presence.rs`).
//!
//! # Failure modes
//! - **stdout closed / broken pipe**: further sends are no-ops. The
//!   shell owning the pipe is authoritative — if it has gone away,
//!   the runtime should not try to reopen or panic.
//! - **serialize failure**: logged and dropped. `Event` variants are
//!   all `Serialize` so this is a "should never happen" but must not
//!   panic if it somehow does.

use std::io::{self, Write};
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::thread;

use presence_ipc::{Event, EventEnvelope};

const OPT_IN_ENV: &str = "PRESENCE_STDOUT_IPC";

/// Hand this to `App` at construction time. `send` from any thread;
/// the writer thread handles encoding and the actual pipe I/O.
#[derive(Clone)]
pub struct EventSink {
    tx: Option<Sender<Event>>,
}

impl EventSink {
    /// Spawn a writer thread iff [`OPT_IN_ENV`] is truthy. Otherwise
    /// returns a sink whose [`send`] method is a no-op — the dev
    /// harness path.
    pub fn spawn_if_enabled() -> Self {
        if !opted_in() {
            return Self { tx: None };
        }
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("presence-ipc-stdout".into())
            .spawn(move || run(io::stdout().lock(), rx))
            .expect("failed to spawn stdout ipc thread");
        log::info!(
            "presence-runtime: {OPT_IN_ENV}=1 — stdout ipc thread started, \
             emitting newline-delimited presence-ipc event envelopes"
        );
        Self { tx: Some(tx) }
    }

    /// A disabled sink for tests. Never emits.
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn disabled() -> Self {
        Self { tx: None }
    }

    /// Best-effort emit. Never blocks (bounded queue implicitly by
    /// `mpsc::channel` — the writer is a fast serializer, so the
    /// queue depth stays small in practice). Silently drops if the
    /// writer thread has exited.
    pub fn send(&self, event: Event) {
        let Some(tx) = &self.tx else {
            return;
        };
        if let Err(SendError(_)) = tx.send(event) {
            // Writer thread is gone — either stdout closed or the OS
            // is tearing us down. Either way, further sends will fail
            // the same way; not worth logging every time.
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

/// `pub(crate)` for tests. External callers go through `EventSink`.
pub(crate) fn run<W: Write>(mut out: W, rx: Receiver<Event>) {
    while let Ok(event) = rx.recv() {
        let line = match serde_json::to_string(&EventEnvelope::wrap(event)) {
            Ok(s) => s,
            Err(err) => {
                // Should never happen — every `Event` is `Serialize` —
                // but a panic here would kill the runtime for a shell
                // that had already shipped, which is far worse than
                // dropping one event.
                log::warn!("presence-runtime: event serialize failed: {err}");
                continue;
            }
        };
        if writeln!(out, "{line}").is_err() {
            log::warn!("presence-runtime: stdout ipc write failed; writer thread exiting");
            return;
        }
        // Flushing explicitly matters: stdout to a pipe is block-buffered
        // on Windows and Linux, so a shell waiting on line-by-line reads
        // would otherwise see events only when the buffer fills.
        if out.flush().is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_writes_every_event_as_a_versioned_ndjson_envelope() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Ready { x: 10, y: 20 }).unwrap();
        tx.send(Event::Moved { x: 30, y: 40 }).unwrap();
        drop(tx); // signal EOF to `run`

        let mut buf: Vec<u8> = Vec::new();
        run(&mut buf, rx);

        let s = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2, "got {s:?}");
        // Envelope shape is stable, so pin down the tag rather than
        // round-tripping — anything shifting the wire format has to
        // update this test too.
        assert!(
            lines[0].contains(r#""kind":"ready""#),
            "line 0: {}",
            lines[0]
        );
        assert!(
            lines[1].contains(r#""kind":"moved""#),
            "line 1: {}",
            lines[1]
        );
        // Version stamp must be present.
        assert!(lines[0].contains(r#""version":"#), "line 0: {}", lines[0]);
    }
}
