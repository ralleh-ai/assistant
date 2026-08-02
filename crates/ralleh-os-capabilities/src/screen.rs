//! Screen capture capability — trait + mock only (no OS binding yet).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScreenCaptureError {
    #[error("screen capture unavailable: {0}")]
    Unavailable(String),
}

/// RGBA primary-display frame (placeholder shape for future OS backends).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Privileged screen capture. Policy capability: `os.screen.capture`.
pub trait ScreenCapture: Send + Sync {
    fn backend_id(&self) -> &'static str;
    fn capture_primary(&self) -> Result<ScreenFrame, ScreenCaptureError>;
}

/// Deterministic 1×1 teal pixel for headless tests.
#[derive(Debug, Default, Clone)]
pub struct MockScreenCapture;

impl ScreenCapture for MockScreenCapture {
    fn backend_id(&self) -> &'static str {
        "mock"
    }

    fn capture_primary(&self) -> Result<ScreenFrame, ScreenCaptureError> {
        Ok(ScreenFrame {
            width: 1,
            height: 1,
            // teal-ish RGBA matching desktop brand
            rgba: vec![0x1f, 0x8a, 0x7a, 0xff],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_frame_is_one_pixel() {
        let frame = MockScreenCapture.capture_primary().unwrap();
        assert_eq!(frame.width, 1);
        assert_eq!(frame.height, 1);
        assert_eq!(frame.rgba.len(), 4);
    }
}
