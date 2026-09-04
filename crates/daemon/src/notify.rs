//! Desktop notifications, dependency-free.
//!
//! * macOS: `osascript` (built-in)
//! * Linux: `notify-send` (libnotify, standard on Ubuntu/Debian)
//!
//! If the mechanism is unavailable the notification is logged instead —
//! never silently swallowed.

pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        // AppleScript string literals must use double quotes, not single quotes.
        // `spawn()` only means the process started — always check the exit status.
        let script = format!(
            "display notification {} with title {}",
            applescript_string(body),
            applescript_string(title)
        );
        match std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
        {
            Ok(out) if out.status.success() => {
                log::info!("notify: {title} — {body}");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                log::warn!(
                    "osascript notification failed (status {}): {}",
                    out.status,
                    stderr.trim()
                );
            }
            Err(e) => log::warn!("could not run osascript for notification: {e}"),
        }
    }
    #[cfg(target_os = "linux")]
    {
        match std::process::Command::new("notify-send")
            .arg(title)
            .arg(body)
            .output()
        {
            Ok(out) if out.status.success() => {
                log::info!("notify: {title} — {body}");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                log::warn!(
                    "notify-send failed (status {}): {}",
                    out.status,
                    stderr.trim()
                );
            }
            Err(e) => log::warn!("could not run notify-send: {e}"),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        log::info!("notify: {title} — {body}");
    }
}

/// Format a Rust string as an AppleScript double-quoted string literal.
/// Only needed on macOS (and in unit tests that assert escaping).
#[cfg(any(test, target_os = "macos"))]
fn applescript_string(s: &str) -> String {
    let cleaned = s
        .chars()
        .map(|c| match c {
            '—' | '–' | '−' => '-',
            '→' | '←' | '⇒' | '⇐' => '-',
            '“' | '”' | '„' => '"',
            '‘' | '’' => '\'',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect::<String>();
    // AppleScript: "  and  \  must be backslash-escaped inside double quotes.
    let escaped = cleaned.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_string_uses_double_quotes() {
        let s = applescript_string("hello world");
        assert_eq!(s, "\"hello world\"");
    }

    #[test]
    fn applescript_string_escapes_quotes_and_backslashes() {
        let s = applescript_string(r#"say "hi" \ ok"#);
        assert_eq!(s, r#""say \"hi\" \\ ok""#);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn osascript_accepts_generated_notification_script() {
        // Runs REAL osascript: requires an interactive GUI session where the
        // process may send Apple events. Headless/CI sessions and transient
        // notification-center contention make it error intermittently, which
        // would flake the deterministic suite — opt in explicitly.
        if std::env::var("HANDOVER_TEST_OSASCRIPT").is_err() {
            eprintln!("skipping: set HANDOVER_TEST_OSASCRIPT=1 to run the live osascript check");
            return;
        }
        let script = format!(
            "display notification {} with title {}",
            applescript_string("Handover test body"),
            applescript_string("Handover")
        );
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .expect("run osascript");
        assert!(
            out.status.success(),
            "osascript failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
