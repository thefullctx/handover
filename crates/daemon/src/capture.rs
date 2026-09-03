//! Capture sources. Clipboard capture was removed (the palette never reads
//! the clipboard); this module keeps the write side used by "Copy", and the
//! manual text capture helper for programmatic sends.

use std::path::Path;

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

/// The maximum size of a file that is read into a capture (1 MiB).
pub const MAX_FILE_CAPTURE_BYTES: u64 = 1024 * 1024;

/// Maximum UTF-8 byte length of text embedded from clipboard / CLI / stdin.
/// Larger text is truncated and a `text_note` is added (mirrors the file cap).
pub const MAX_TEXT_CAPTURE_BYTES: usize = 1024 * 1024;

/// Truncate oversized text bodies (clipboard / CLI / stdin) so prompts cannot
/// balloon without bound. Sets `metadata["text_note"]` when truncation occurs.
pub(crate) fn cap_text_capture(capture: &mut Capture) {
    let Some(text) = capture.content.text.as_ref() else {
        return;
    };
    let original_bytes = text.len();
    if original_bytes <= MAX_TEXT_CAPTURE_BYTES {
        return;
    }
    // Truncate on a UTF-8 character boundary at or below the byte budget.
    let mut end = MAX_TEXT_CAPTURE_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = text[..end].to_string();
    capture.content.text = Some(truncated);
    capture.metadata.insert(
        "text_note".into(),
        format!(
            "text was truncated to {:.1} MiB (original was {:.1} MiB)",
            MAX_TEXT_CAPTURE_BYTES as f64 / 1_048_576.0,
            original_bytes as f64 / 1_048_576.0
        ),
    );
}

/// Open a path for reading **without following a final-component symlink**,
/// then read up to `max_bytes` from the same handle. Returns metadata from
/// that open file (so a leaf symlink swap after check cannot redirect the read).
///
/// When the file is larger than `max_bytes`, returns `(meta, None)` so the
/// caller can attach a size note without loading the body.
///
/// **Scope of the guarantee:** `O_NOFOLLOW` applies to the last path component
/// only. Intermediate directories are resolved by the kernel during `open(2)`
/// and are not individually pinned with `openat`. See `enrich_file_capture`
/// for the product threat model (trusted parent directories).
///
/// Public so the desktop shell can route palette drops through the exact same
/// reader as daemon file capture — one privacy discipline, both entrances.
pub fn read_file_nofollow(
    path: &Path,
    max_bytes: u64,
) -> Result<(std::fs::Metadata, Option<Vec<u8>>), String> {
    use std::io::Read;

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| {
                let path_disp = path.display();
                if e.raw_os_error() == Some(libc::ELOOP)
                    || e.kind() == std::io::ErrorKind::InvalidInput
                {
                    // macOS/Linux: O_NOFOLLOW on a symlink → ELOOP (or platform variant).
                    format!(
                        "Refusing to attach `{path_disp}` — symbolic links are not allowed for file capture (privacy)."
                    )
                } else {
                    format!("Could not read `{path_disp}`: {e}")
                }
            })?
    };

    #[cfg(not(unix))]
    let mut file = {
        // Best-effort: refuse if lstat says symlink, then open. Windows has no
        // portable O_NOFOLLOW equivalent in std; TOCTOU remains a residual risk.
        let meta = std::fs::symlink_metadata(path)
            .map_err(|e| format!("Could not read `{}`: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "Refusing to attach `{}` — symbolic links are not allowed for file capture (privacy).",
                path.display()
            ));
        }
        std::fs::File::open(path)
            .map_err(|e| format!("Could not read `{}`: {e}", path.display()))?
    };

    let meta = file
        .metadata()
        .map_err(|e| format!("Could not stat `{}`: {e}", path.display()))?;
    if !meta.is_file() {
        return Ok((meta, None));
    }
    if meta.len() > max_bytes {
        return Ok((meta, None));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read `{}`: {e}", path.display()))?;
    Ok((meta, Some(bytes)))
}
