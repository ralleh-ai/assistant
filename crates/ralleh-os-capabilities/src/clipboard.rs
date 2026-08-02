//! Clipboard read/write capability.

use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard unavailable: {0}")]
    Unavailable(String),
    #[error("clipboard empty or non-text")]
    EmptyOrNonText,
}

/// Privileged clipboard access. Callers must have already passed policy
/// (capability `os.clipboard.read` / `os.clipboard.write`).
pub trait Clipboard: Send + Sync {
    fn backend_id(&self) -> &'static str;
    fn read_text(&self) -> Result<String, ClipboardError>;
    fn write_text(&self, text: &str) -> Result<(), ClipboardError>;
}

/// In-memory clipboard for tests and headless smoke.
#[derive(Debug, Clone, Default)]
pub struct MockClipboard {
    slot: Arc<Mutex<String>>,
}

impl MockClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            slot: Arc::new(Mutex::new(text.into())),
        }
    }
}

impl Clipboard for MockClipboard {
    fn backend_id(&self) -> &'static str {
        "mock"
    }

    fn read_text(&self) -> Result<String, ClipboardError> {
        let guard = self
            .slot
            .lock()
            .map_err(|e| ClipboardError::Unavailable(e.to_string()))?;
        if guard.is_empty() {
            return Err(ClipboardError::EmptyOrNonText);
        }
        Ok(guard.clone())
    }

    fn write_text(&self, text: &str) -> Result<(), ClipboardError> {
        let mut guard = self
            .slot
            .lock()
            .map_err(|e| ClipboardError::Unavailable(e.to_string()))?;
        *guard = text.to_string();
        Ok(())
    }
}

#[cfg(feature = "clipboard-os")]
mod system {
    use super::{Clipboard, ClipboardError};

    /// System clipboard via `arboard` (feature `clipboard-os`).
    #[derive(Debug, Default)]
    pub struct SystemClipboard;

    impl SystemClipboard {
        pub fn new() -> Self {
            Self
        }
    }

    impl Clipboard for SystemClipboard {
        fn backend_id(&self) -> &'static str {
            "os"
        }

        fn read_text(&self) -> Result<String, ClipboardError> {
            let mut board = arboard::Clipboard::new()
                .map_err(|e| ClipboardError::Unavailable(e.to_string()))?;
            match board.get_text() {
                Ok(t) if !t.is_empty() => Ok(t),
                Ok(_) => Err(ClipboardError::EmptyOrNonText),
                Err(e) => Err(ClipboardError::Unavailable(e.to_string())),
            }
        }

        fn write_text(&self, text: &str) -> Result<(), ClipboardError> {
            let mut board = arboard::Clipboard::new()
                .map_err(|e| ClipboardError::Unavailable(e.to_string()))?;
            board
                .set_text(text.to_string())
                .map_err(|e| ClipboardError::Unavailable(e.to_string()))
        }
    }
}

#[cfg(feature = "clipboard-os")]
pub use system::SystemClipboard;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_roundtrip() {
        let clip = MockClipboard::new();
        clip.write_text("ralleh").unwrap();
        assert_eq!(clip.read_text().unwrap(), "ralleh");
        assert_eq!(clip.backend_id(), "mock");
    }

    #[test]
    fn mock_empty_read_errors() {
        let clip = MockClipboard::new();
        assert!(matches!(
            clip.read_text(),
            Err(ClipboardError::EmptyOrNonText)
        ));
    }
}
