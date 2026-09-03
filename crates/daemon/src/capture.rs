//! Capture sources. Clipboard capture was removed (the palette never reads
//! the clipboard); this module keeps the write side used by "Copy", and the
//! manual text capture helper for programmatic sends.

use arboard::Clipboard;
use handover_core::capture::{Capture, SourceKind};

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("Unable to access the clipboard.")]
    Clipboard,
    #[error("{0}")]
    Other(String),
}

pub fn set_clipboard_text(text: &str) -> Result<(), CaptureError> {
    let mut clipboard = Clipboard::new().map_err(|_| CaptureError::Clipboard)?;
    clipboard
        .set_text(text.to_string())
        .map_err(|_| CaptureError::Clipboard)
}

pub fn capture_from_text(text: String) -> Capture {
    let mut capture = Capture::text(SourceKind::Manual, text, None);
    capture
        .metadata
        .insert("capture_method".to_string(), "cli".to_string());
    capture
}
