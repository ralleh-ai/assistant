//! Rotated text log for `presence-runtime`'s stderr.
//!
//! The runtime's own logs (`log::info!` / panic messages / wgpu
//! validation errors) previously went to the shell's inherited
//! terminal, where they were useful during dev and invisible in
//! production. This module captures them to a rotated file under
//! the Tauri app config dir so a `PresenceStalled` audit event
//! points to a real correlation trail.
//!
//! # Design notes
//!
//! - Text log, not JSON. The runtime does not emit structured
//!   events; every attempt to force one would be lossy (some of
//!   the most useful lines here are panics and driver warnings
//!   that the runtime itself did not choose to format).
//! - Size-based rotation (4 MiB, one rollover) mirrors the audit
//!   log so operators don't have to learn a second rotation
//!   policy. Older content is dropped rather than compressed —
//!   an ops person tailing this file wants "the last few
//!   minutes", not "everything since install".
//! - Fail-open: if the file cannot be opened, the reader thread
//!   forwards each line to `log::info!` with a prefix so nothing
//!   is silently lost.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

/// Same threshold as the audit log; the two rotate on the same
/// scale and produce roughly the same on-disk footprint.
const ROTATE_AT_BYTES: u64 = 4 * 1024 * 1024;
const LOG_FILENAME: &str = "presence.log";
const ROLLOVER_FILENAME: &str = "presence.log.1";

pub struct PresenceLog {
    inner: Mutex<Inner>,
    /// `true` for the no-op sink (open() failed). Writes short-
    /// circuit; the stderr reader thread falls back to `log::info!`
    /// so nothing is dropped on the floor.
    disabled: bool,
}

struct Inner {
    dir: PathBuf,
    active: PathBuf,
    rollover: PathBuf,
}

impl PresenceLog {
    /// Open the log under the Tauri app config dir. On failure
    /// returns a disabled sink — callers should not treat that as
    /// a startup blocker (the runtime works fine without file
    /// capture, and `log::info!` still fires).
    pub fn open(app: &AppHandle) -> Self {
        let dir = match app.path().app_config_dir() {
            Ok(d) => d,
            Err(e) => {
                log::warn!("presence-log: no app config dir ({e}); disabling capture");
                return Self::disabled();
            }
        };
        if let Err(e) = fs::create_dir_all(&dir) {
            log::warn!(
                "presence-log: cannot create {} ({e}); disabling capture",
                dir.display()
            );
            return Self::disabled();
        }
        Self::for_dir(dir)
    }

    fn for_dir(dir: PathBuf) -> Self {
        let active = dir.join(LOG_FILENAME);
        let rollover = dir.join(ROLLOVER_FILENAME);
        Self {
            inner: Mutex::new(Inner {
                dir,
                active,
                rollover,
            }),
            disabled: false,
        }
    }

    fn disabled() -> Self {
        let mut s = Self::for_dir(PathBuf::from("__disabled__"));
        s.disabled = true;
        s
    }

    #[allow(dead_code)] // reserved for a future settings-UI "capture status" indicator
    pub fn is_enabled(&self) -> bool {
        !self.disabled
    }

    /// Absolute path of the currently-active file (diagnostics
    /// only — operators tail it directly).
    pub fn active_path(&self) -> Option<PathBuf> {
        if self.disabled {
            return None;
        }
        self.inner.lock().ok().map(|g| g.active.clone())
    }

    /// Append `line` (without trailing newline; we add one). No
    /// timestamp prefix here because the runtime's own log lines
    /// carry one from `env_logger`; adding a second timestamp
    /// would be redundant and misleading for panic traces that
    /// span multiple lines.
    pub fn write_line(&self, line: &str) -> Result<(), String> {
        if self.disabled {
            return Err("presence log disabled".into());
        }
        let mut guard = self.inner.lock().map_err(|e| e.to_string())?;
        if !guard.dir.exists() {
            fs::create_dir_all(&guard.dir)
                .map_err(|e| format!("presence-log: recreate dir: {e}"))?;
        }
        // Rotate before write when the *next* write would push us
        // over the cap — matches audit.rs so operators see the
        // same "at most 4 MiB active + 4 MiB rollover" ceiling.
        let projected = line.len() as u64 + 1;
        if let Ok(meta) = fs::metadata(&guard.active) {
            if meta.len() + projected > ROTATE_AT_BYTES {
                rotate(&mut guard)?;
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&guard.active)
            .map_err(|e| format!("presence-log: open {}: {e}", guard.active.display()))?;
        writeln!(file, "{line}").map_err(|e| format!("presence-log: write: {e}"))?;
        Ok(())
    }

    /// Read the last `limit` non-empty lines from the active file.
    /// Older content in the rollover is not returned — the intent
    /// is "what did the runtime say recently?", not a full audit.
    pub fn tail(&self, limit: usize) -> Result<Vec<String>, String> {
        if self.disabled {
            return Ok(Vec::new());
        }
        let guard = self.inner.lock().map_err(|e| e.to_string())?;
        let raw = match fs::read_to_string(&guard.active) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("presence-log: read: {e}")),
        };
        let lines: Vec<String> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        let start = lines.len().saturating_sub(limit);
        Ok(lines[start..].to_vec())
    }
}

fn rotate(inner: &mut Inner) -> Result<(), String> {
    let _ = fs::remove_file(&inner.rollover);
    if inner.active.exists() {
        fs::rename(&inner.active, &inner.rollover)
            .map_err(|e| format!("presence-log: rotate: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_creates_the_file_and_appends() {
        let dir = TempDir::new().unwrap();
        let log = PresenceLog::for_dir(dir.path().to_path_buf());
        log.write_line("first line").unwrap();
        log.write_line("second line").unwrap();
        let read = fs::read_to_string(dir.path().join(LOG_FILENAME)).unwrap();
        assert!(read.contains("first line"));
        assert!(read.contains("second line"));
        // Newline-separated so each entry is one physical line.
        assert_eq!(read.matches('\n').count(), 2);
    }

    #[test]
    fn rotation_moves_active_to_rollover_when_over_cap() {
        let dir = TempDir::new().unwrap();
        let log = PresenceLog::for_dir(dir.path().to_path_buf());
        // Force the active file just above the cap so the next
        // write triggers rotation without us actually writing
        // 4 MiB — that would slow the test surface.
        let path = dir.path().join(LOG_FILENAME);
        let filler = "x".repeat((ROTATE_AT_BYTES + 100) as usize);
        fs::write(&path, filler).unwrap();
        log.write_line("post-rotate").unwrap();
        assert!(dir.path().join(ROLLOVER_FILENAME).exists());
        let active = fs::read_to_string(&path).unwrap();
        assert_eq!(active.trim(), "post-rotate");
    }

    #[test]
    fn tail_returns_only_the_last_n_non_empty_lines() {
        let dir = TempDir::new().unwrap();
        let log = PresenceLog::for_dir(dir.path().to_path_buf());
        log.write_line("a").unwrap();
        log.write_line("").unwrap();
        log.write_line("b").unwrap();
        log.write_line("c").unwrap();
        let tail = log.tail(2).unwrap();
        assert_eq!(tail, vec!["b".to_string(), "c".to_string()]);
    }

    #[test]
    fn tail_returns_empty_when_log_does_not_exist_yet() {
        let dir = TempDir::new().unwrap();
        let log = PresenceLog::for_dir(dir.path().to_path_buf());
        assert!(log.tail(10).unwrap().is_empty());
    }

    #[test]
    fn disabled_sink_never_touches_disk() {
        let log = PresenceLog::disabled();
        assert!(!log.is_enabled());
        assert!(log.write_line("nope").is_err());
        assert!(log.tail(10).unwrap().is_empty());
        assert!(!PathBuf::from("__disabled__").exists());
    }
}
