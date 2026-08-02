//! Global hotkey registration — trait + mock only (no OS binding yet).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("hotkey unavailable: {0}")]
    Unavailable(String),
    #[error("hotkey already registered: {0}")]
    AlreadyRegistered(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HotkeyCombo {
    pub modifiers: Vec<&'static str>,
    pub key: &'static str,
}

/// Privileged hotkey registration. Policy capability: `os.hotkey.register`.
pub trait HotkeyRegistrar: Send + Sync {
    fn backend_id(&self) -> &'static str;
    fn register(&self, id: &str, combo: HotkeyCombo) -> Result<(), HotkeyError>;
    fn unregister(&self, id: &str) -> Result<(), HotkeyError>;
}

#[derive(Debug, Clone, Default)]
pub struct MockHotkeyRegistrar {
    ids: Arc<Mutex<HashSet<String>>>,
}

impl MockHotkeyRegistrar {
    pub fn new() -> Self {
        Self::default()
    }
}

impl HotkeyRegistrar for MockHotkeyRegistrar {
    fn backend_id(&self) -> &'static str {
        "mock"
    }

    fn register(&self, id: &str, _combo: HotkeyCombo) -> Result<(), HotkeyError> {
        let mut guard = self
            .ids
            .lock()
            .map_err(|e| HotkeyError::Unavailable(e.to_string()))?;
        if !guard.insert(id.to_string()) {
            return Err(HotkeyError::AlreadyRegistered(id.to_string()));
        }
        Ok(())
    }

    fn unregister(&self, id: &str) -> Result<(), HotkeyError> {
        let mut guard = self
            .ids
            .lock()
            .map_err(|e| HotkeyError::Unavailable(e.to_string()))?;
        guard.remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_register_once() {
        let hk = MockHotkeyRegistrar::new();
        let combo = HotkeyCombo {
            modifiers: vec!["ctrl", "shift"],
            key: "Space",
        };
        hk.register("ptt", combo.clone()).unwrap();
        assert!(matches!(
            hk.register("ptt", combo),
            Err(HotkeyError::AlreadyRegistered(_))
        ));
    }
}
