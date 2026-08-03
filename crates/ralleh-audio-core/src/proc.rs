//! Timeout-bounded child-process execution for the CLI STT/TTS bridges.
//!
//! `std::process::Child::wait`/`Command::output` block forever. A wedged
//! `whisper-cli` or `piper` (bad model, deadlocked GPU driver, waiting on
//! input that never comes) would otherwise hang the voice pipeline for the
//! life of the process. This helper waits with a wall-clock deadline and kills
//! the child if it overruns, while draining stdout/stderr on separate threads
//! so a child that fills a pipe buffer can't deadlock against our wait loop.

use std::io::Read;
use std::process::{Child, Output};
use std::time::{Duration, Instant};

/// Wait for `child` to exit, killing it if it exceeds `timeout`. stdout/stderr
/// are drained concurrently to avoid pipe-buffer deadlock. On timeout the child
/// is killed and reaped and an `Err` describing the overrun is returned.
pub(crate) fn wait_with_timeout(mut child: Child, timeout: Duration) -> Result<Output, String> {
    // Take the pipe handles and drain them on dedicated threads. A child that
    // writes more than a pipe buffer (~64 KiB) would block on write if we only
    // polled `try_wait`, never making progress — classic deadlock.
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_reader = std::thread::spawn(move || drain(stdout));
    let err_reader = std::thread::spawn(move || drain(stderr));

    let start = Instant::now();
    let poll = Duration::from_millis(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "process exceeded {}s timeout and was killed",
                        timeout.as_secs()
                    ));
                }
                std::thread::sleep(poll);
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to wait on child process: {e}"));
            }
        }
    };

    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn drain(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut buf);
    }
    buf
}
