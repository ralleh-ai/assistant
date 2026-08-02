//! Desktop OS capabilities — traits + mocks first (threat model T13).
//!
//! Real OS bindings stay behind Cargo features so default
//! `cargo test --workspace` never needs a display, clipboard, or input
//! device. See `/docs/HEADLESS.md`.

mod clipboard;
mod hotkey;
mod screen;

pub use clipboard::{Clipboard, ClipboardError, MockClipboard};
pub use hotkey::{HotkeyCombo, HotkeyError, HotkeyRegistrar, MockHotkeyRegistrar};
pub use screen::{MockScreenCapture, ScreenCapture, ScreenCaptureError, ScreenFrame};

#[cfg(feature = "clipboard-os")]
pub use clipboard::SystemClipboard;
