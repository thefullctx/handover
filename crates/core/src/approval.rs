//! Approval detection — the ONE documented exception to the privacy rule.
//!
//! Discovery and status read filenames + mtimes only, never transcript
//! contents. But an agent paused at a permission prompt is *quiet* — exactly
//! like an idle agent — so it cannot be distinguished by mtime alone. The
//! only honest signal lives in the transcript's tail (a permission-marker
//! line). This module implements that single, narrow, opt-in read:
//!
//! * Only the LAST few KB of a live session's transcript are read.
//! * Only agents with a `permission_marker` configured are ever peeked at.
//! * The buffer is scanned for the marker and then dropped — contents are
//!   never stored, logged, or sent anywhere.
//!
//! Nothing in this module runs unless the daemon opts an agent in.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How much of a transcript tail is read when looking for a permission
/// marker. Small on purpose: the marker line sits at the end of an active
/// transcript, and the read is the privacy exception.
pub const APPROVAL_PEEK_BYTES: u64 = 8 * 1024;

/// Reads the last [`APPROVAL_PEEK_BYTES`] of `path` as a lossy string.
/// Best-effort: a missing/unreadable file or an empty file returns `None`.
/// The buffer is handed to the caller and dropped — contents are never
/// stored, logged, or sent anywhere beyond the returned value.
///
/// Symlinks are refused (`O_NOFOLLOW` on Unix, `symlink_metadata` pre-check
/// elsewhere) to match the file-capture discipline.
fn read_tail(path: &Path) -> Option<String> {
    // Refuse symlinks without following them.
    let sym = std::fs::symlink_metadata(path).ok()?;
    if !sym.file_type().is_file() {
        return None;
    }
    let len = sym.len();
    if len == 0 {
        return None;
    }
    let skip = len.saturating_sub(APPROVAL_PEEK_BYTES);
    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .ok()?
    };
    #[cfg(not(unix))]
    let mut file = std::fs::File::open(path).ok()?;
    if skip > 0 && file.seek(SeekFrom::Start(skip)).is_err() {
        return None;
    }
    let mut buf = Vec::new();
    if file
        .take(APPROVAL_PEEK_BYTES)
        .read_to_end(&mut buf)
        .is_err()
    {
        return None;
    }
    // Lossy conversion is fine: the marker is ASCII/UTF-8 and the buffer is
    // dropped immediately after the check.
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// True when the marker appears within the last [`APPROVAL_PEEK_BYTES`]
/// bytes of `path`'s contents. Read-only, best-effort: a missing/unreadable
/// file, or a marker straddling the read boundary, reports `false` (fail
/// closed — never claim blocked on a partial read).
pub fn tail_contains_marker(path: &Path, marker: &str) -> bool {
    !marker.is_empty() && read_tail(path).is_some_and(|tail| tail.contains(marker))
}

/// The single line containing the marker, trimmed — what the agent is
/// asking right now (the approval card shows it to the user). `None` when
/// the marker is absent, the file is unreadable, or the line is empty.
/// The line is capped so a runaway log line cannot bloat the payload.
pub fn tail_marker_line(path: &Path, marker: &str) -> Option<String> {
    if marker.is_empty() {
        return None;
    }
    let tail = read_tail(path)?;
    let line = tail.lines().find(|l| l.contains(marker))?.trim();
    if line.is_empty() {
        return None;
    }
    let capped: String = line.chars().take(APPROVAL_LINE_CAP).collect();
    Some(capped)
}

/// Cap for the surfaced marker line (the permission prompt itself).
const APPROVAL_LINE_CAP: usize = 200;

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(contents: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ho-approval-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("transcript.jsonl");
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn marker_near_end_is_found() {
        let path = temp_file(
            "user: do the thing\nassistant: working...\n[permission] approve shell command?\n",
        );
        assert!(tail_contains_marker(&path, "[permission]"));
    }

    #[test]
    fn marker_deep_in_history_is_not_seen() {
        // A marker from an earlier approval sits beyond the peek window.
        let mut contents = String::new();
        for i in 0..2000 {
            contents.push_str(&format!("line {i} of filler text that is not a marker\n"));
        }
        contents.push_str("[permission] old approval\n");
        contents.push_str(&"assistant: continuing silently\n".repeat(2000));
        let path = temp_file(&contents);
        assert!(!tail_contains_marker(&path, "[permission]"));
    }

    #[test]
    fn fresh_marker_after_quiet_tail_is_found() {
        let mut contents = String::new();
        contents.push_str(&"assistant: thinking quietly\n".repeat(2000));
        contents.push_str("[permission] approve?\n");
        let path = temp_file(&contents);
        assert!(tail_contains_marker(&path, "[permission]"));
    }

    #[test]
    fn empty_marker_never_matches() {
        let path = temp_file("anything at all\n");
        assert!(!tail_contains_marker(&path, ""));
    }

    #[test]
    fn missing_file_fails_closed() {
        assert!(!tail_contains_marker(
            std::path::Path::new("/nonexistent/ho-approval/transcript.jsonl"),
            "[permission]"
        ));
    }

    // -- tail_marker_line ------------------------------------------------

    #[test]
    fn marker_line_is_the_prompt_the_agent_is_asking() {
        let path =
            temp_file("user: do the thing\n[permission] approve shell command: rm -rf /tmp/x?\n");
        assert_eq!(
            tail_marker_line(&path, "[permission]").as_deref(),
            Some("[permission] approve shell command: rm -rf /tmp/x?")
        );
    }

    #[test]
    fn marker_line_trims_surrounding_whitespace() {
        let path = temp_file("assistant: thinking\n  [permission] approve?  \n");
        assert_eq!(
            tail_marker_line(&path, "[permission]").as_deref(),
            Some("[permission] approve?")
        );
    }

    #[test]
    fn marker_line_is_capped() {
        let long = format!("{} {}", "[permission]", "y".repeat(500));
        let path = temp_file(&long);
        let line = tail_marker_line(&path, "[permission]").expect("marker line");
        assert!(line.chars().count() <= 200);
        assert!(line.starts_with("[permission]"));
    }

    #[test]
    fn marker_line_none_when_not_blocked() {
        let path = temp_file("assistant: happily working\n");
        assert_eq!(tail_marker_line(&path, "[permission]"), None);
    }

    #[test]
    fn marker_line_none_when_marker_empty_or_file_missing() {
        let path = temp_file("[permission] approve?\n");
        assert_eq!(tail_marker_line(&path, ""), None);
        assert_eq!(
            tail_marker_line(
                std::path::Path::new("/nonexistent/ho-approval/x.jsonl"),
                "[permission]"
            ),
            None
        );
    }
}
