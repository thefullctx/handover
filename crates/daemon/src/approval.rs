//! Approval layer — acting on a blocked session (Phase 6.5).
//!
//! Detection lives in core (`tail_contains_marker`): for agents with a
//! `permission_marker` configured, the daemon reads ONLY the tail of a live
//! session's transcript to learn it is parked at a permission prompt.
//! This module builds the injection command for the configured channel and
//! VERIFIES the outcome by watching the transcript resume (mtime advances or
//! the marker disappears). Success is never claimed without verification —
//! if the transcript does not move within the budget, the caller gets a
//! fail-soft result ("couldn't confirm — check the terminal").
//!
//! SAFETY: injection is opt-in per agent (`approval_channel`), requires an
//! explicit `approval_target`, and re-checks that the session is STILL
//! blocked immediately before injecting (never inject into a session that
//! moved on). The `agent` channel is deliberately unimplemented — it fails
//! loud instead of guessing an agent-specific protocol.
//!
//! Residual risk, accepted: detection trusts the transcript tail, so a
//! hostile agent can print the marker itself and appear blocked. The
//! ceiling of that spoof is a spoofed prompt — injection only ever sends
//! a single `y`/`n` keystroke to a validated, user-configured target, and
//! the user still clicks Approve/Deny explicitly.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use handover_config::ApprovalChannel;

use handover_core::approval::tail_contains_marker;

use crate::status::{compute_live_sessions, LiveSessionsSnapshot};
use crate::DaemonError;

/// How long to watch a transcript after injecting before failing soft.
pub const APPROVAL_VERIFY_BUDGET: Duration = Duration::from_secs(10);

/// Result of an approval action. `verified` is only true when the transcript
/// was observed resuming — anything less is reported, never claimed.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApprovalResult {
    pub verified: bool,
    pub message: String,
}

/// Builds the injection command for a channel + target. Pure and testable.
///
/// * `tty`   → `sh -c 'printf %s <y|n> > <device>'` — one keystroke into the
///   session's tty device (the target from `handover attach`). The target must
///   look like a terminal device (`/dev/tty*`, `/dev/pts/*`) and is
///   single-quoted — an arbitrary string is rejected, never interpolated.
/// * `tmux`  → `tmux send-keys -t <target> <y|n>` (no shell involved).
/// * `agent` → unimplemented (fail loud, never guess).
pub fn build_approval_command(
    channel: &ApprovalChannel,
    target: &str,
    approve: bool,
) -> Result<Vec<String>, String> {
    let key = if approve { "y" } else { "n" };
    match channel {
        ApprovalChannel::Tty => {
            let trimmed = target.trim();
            if trimmed.is_empty() {
                return Err(
                    "tty approval channel needs an approval_target (a device like /dev/ttys002). \
                     Run `handover attach` inside the session, or set approval_target in config."
                        .to_string(),
                );
            }
            if !is_tty_device_path(trimmed) {
                return Err(format!(
                    "`{trimmed}` is not a tty device path — approval_target must look like \
                     /dev/ttys002 or /dev/pts/0. Nothing was injected."
                ));
            }
            Ok(vec![
                "sh".into(),
                "-c".into(),
                format!(
                    "printf %s {} > {}",
                    shell_single_quote(key),
                    shell_single_quote(trimmed)
                ),
            ])
        }
        ApprovalChannel::Tmux => {
            let target = target.trim();
            if target.is_empty() {
                return Err(
                    "tmux approval channel needs an approval_target (the tmux target/pane). \
                     Set approval_target in config."
                        .to_string(),
                );
            }
            if !is_safe_tmux_target(target) {
                return Err(format!(
                    "`{target}` is not a safe tmux target — must not start with `-` \
                     and must not contain control characters or shell metacharacters. \
                     Nothing was injected."
                ));
            }
            Ok(vec![
                "tmux".into(),
                "send-keys".into(),
                "-t".into(),
                target.into(),
                key.into(),
            ])
        }
        ApprovalChannel::Agent => Err(
            "the `agent` approval channel is not implemented yet — use `tty` or `tmux`. \
             Nothing was injected."
                .to_string(),
        ),
    }
}

/// Single-quote-escapes a string for shell use (embedded in `sh -c`).
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Is this a safe tmux `send-keys -t` target? The command uses argv (no
/// shell), so shell metacharacters cannot execute — but a target starting
/// with `-` would parse as a tmux flag. Reject leading dashes, control
/// characters, and shell metacharacters outright.
fn is_safe_tmux_target(s: &str) -> bool {
    if s.is_empty() || s.starts_with('-') || s.starts_with(' ') {
        return false;
    }
    !s.chars().any(|c| {
        c.is_control()
            || matches!(
                c,
                ';' | '|' | '&' | '$' | '`' | '(' | ')' | '<' | '>' | '\\' | '"' | '\''
            )
    })
}

/// Does this look like a terminal device path? Defense-in-depth for the tty
/// approval channel: even though the target is shell-quoted, only accept the
/// shapes ttys actually take (`/dev/ttyNNN`, `/dev/pts/NNN`) — and the final
/// component must be purely alphanumeric, so `ttys002; id` cannot sneak through.
fn is_tty_device_path(s: &str) -> bool {
    let p = std::path::Path::new(s);
    let alnum_tail =
        |name: &str| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric());
    match (
        p.parent().and_then(|d| d.to_str()),
        p.file_name().and_then(|f| f.to_str()),
    ) {
        (Some("/dev"), Some(name)) => name.starts_with("tty") && alnum_tail(name),
        (Some("/dev/pts"), Some(name)) => alnum_tail(name),
        _ => false,
    }
}

/// Polls a transcript until it shows signs of life: the marker is gone from
/// the tail, or the mtime advanced past `baseline`. Returns false when the
/// budget expires (fail soft).
pub fn verify_session_resumed(
    path: &Path,
    marker: &str,
    baseline: SystemTime,
    budget: Duration,
) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if !tail_contains_marker(path, marker) {
            return true;
        }
        if let Ok(meta) = std::fs::metadata(path) {
            if let Ok(m) = meta.modified() {
                if m > baseline {
                    return true;
                }
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// A resolved approval request, ready for [`Daemon::execute_approval`].
/// Carries everything the slow path needs so it can run *outside* the
/// daemon lock: the injection command, the transcript path, the marker,
/// and the baseline mtime for verification.
pub struct ApprovalRequest {
    pub agent_id: String,
    pub session_id: String,
    pub approve: bool,
    pub path: PathBuf,
    pub marker: String,
    pub cmd: Vec<String>,
    pub baseline: Option<std::time::SystemTime>,
}

/// Fast (lock-held) half of an approval: validated config and the session
/// snapshot. [`complete_approval`] re-checks liveness/blocking and builds
/// the [`ApprovalRequest`] off the lock.
pub struct ApprovalPlan {
    pub agent_id: String,
    pub session_id: String,
    pub approve: bool,
    pub marker: String,
    pub channel: ApprovalChannel,
    pub target: String,
    pub snapshot: LiveSessionsSnapshot,
}

/// Slow half of an approval, off the daemon lock: re-check the session is
/// live and still blocked, then build the injection command. Returns an
/// [`ApprovalRequest`] ready for [`execute_approval`].
pub fn complete_approval(plan: ApprovalPlan) -> Result<ApprovalRequest, DaemonError> {
    let msg = |m: String| DaemonError::Message(m);

    // The session must be live AND have a transcript to verify.
    let sessions = compute_live_sessions(&plan.snapshot);
    let session = sessions
        .iter()
        .find(|s| s.agent_id == plan.agent_id && s.session_id == plan.session_id)
        .ok_or_else(|| {
            msg(format!(
                "Session `{}` for `{}` is not live.",
                plan.session_id, plan.agent_id
            ))
        })?;
    let path = session.path.as_ref().ok_or_else(|| {
        msg(format!(
            "Session `{}` has no transcript (cli-list discovery) — approval is \
             only supported for agents with session files.",
            plan.session_id
        ))
    })?;

    // Race safety: never inject into a session that moved on.
    if !tail_contains_marker(path, &plan.marker) {
        return Err(msg(format!(
            "Session `{}` is no longer blocked — the agent moved on. Nothing was injected.",
            plan.session_id
        )));
    }

    // Build the injection command.
    let cmd = build_approval_command(&plan.channel, &plan.target, plan.approve)
        .map_err(DaemonError::Message)?;
    let baseline = std::fs::metadata(path).and_then(|m| m.modified()).ok();

    Ok(ApprovalRequest {
        agent_id: plan.agent_id,
        session_id: plan.session_id,
        approve: plan.approve,
        path: path.clone(),
        marker: plan.marker,
        cmd,
        baseline,
    })
}

/// Executes a resolved approval: injects the keystroke and polls the
/// transcript for signs of life. A free function (not a method on `Daemon`)
/// so callers can run it *without* the daemon lock — injection + verify
/// polling must never block the palette, the API, or other handoffs.
pub fn execute_approval(
    req: &ApprovalRequest,
    budget: std::time::Duration,
) -> Result<ApprovalResult, DaemonError> {
    let verb = if req.approve { "Approved" } else { "Denied" };
    let status = std::process::Command::new(&req.cmd[0])
        .args(&req.cmd[1..])
        .status();
    match status {
        Ok(s) if s.success() => {
            let resumed = req.baseline.is_some()
                && verify_session_resumed(&req.path, &req.marker, req.baseline.unwrap(), budget);
            if resumed {
                Ok(ApprovalResult {
                    verified: true,
                    message: format!("{verb} — the agent is working again."),
                })
            } else {
                Ok(ApprovalResult {
                    verified: false,
                    message: format!(
                        "{verb} — decision sent, but couldn't confirm the agent resumed. \
                         Check the terminal."
                    ),
                })
            }
        }
        Ok(s) => Err(DaemonError::Message(format!(
            "Approval injection failed (exit {s}). Nothing was sent."
        ))),
        Err(e) => Err(DaemonError::Message(format!(
            "Could not run approval injection: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_command_writes_single_keystroke_to_device() {
        let cmd = build_approval_command(&ApprovalChannel::Tty, "/dev/ttys002", true).unwrap();
        assert_eq!(cmd[0], "sh");
        assert_eq!(cmd[2], "printf %s 'y' > '/dev/ttys002'");
        let deny = build_approval_command(&ApprovalChannel::Tty, "/dev/ttys002", false).unwrap();
        assert_eq!(deny[2], "printf %s 'n' > '/dev/ttys002'");
        // Linux pty shape is accepted too.
        assert!(build_approval_command(&ApprovalChannel::Tty, "/dev/pts/3", true).is_ok());
    }

    #[test]
    fn tty_target_must_look_like_a_tty_device_injection_is_refused() {
        // Command injection via the target must never reach the shell.
        let evil = "/dev/ttys002; id";
        let err = build_approval_command(&ApprovalChannel::Tty, evil, true)
            .expect_err("injection target must be rejected");
        assert!(err.contains("not a tty device path"), "{err}");
        // Spaces (accidental two-token paths) are rejected as well.
        assert!(build_approval_command(&ApprovalChannel::Tty, "/dev/tty s002", true).is_err());
        // Arbitrary non-device strings too.
        assert!(build_approval_command(&ApprovalChannel::Tty, "some/random/path", true).is_err());
        assert!(build_approval_command(&ApprovalChannel::Tty, "../escape", true).is_err());
    }

    #[test]
    fn tmux_command_targets_the_pane() {
        let cmd = build_approval_command(&ApprovalChannel::Tmux, "work:0.1", true).unwrap();
        assert_eq!(
            cmd,
            vec![
                "tmux".to_string(),
                "send-keys".to_string(),
                "-t".to_string(),
                "work:0.1".to_string(),
                "y".to_string()
            ]
        );
    }

    #[test]
    fn missing_target_fails_loud() {
        assert!(build_approval_command(&ApprovalChannel::Tty, "", true).is_err());
        assert!(build_approval_command(&ApprovalChannel::Tmux, "", true).is_err());
    }

    #[test]
    fn agent_channel_is_unimplemented_not_guessed() {
        assert!(build_approval_command(&ApprovalChannel::Agent, "x", true).is_err());
    }

    #[test]
    fn verify_sees_mtime_advance() {
        let dir = std::env::temp_dir().join(format!("ho-verify-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, "[permission] approve?\n").unwrap();
        let baseline = std::fs::metadata(&path).unwrap().modified().unwrap();

        // Simulate the agent resuming: append to the transcript.
        std::thread::spawn({
            let path = path.clone();
            move || {
                std::thread::sleep(Duration::from_millis(100));
                let mut f = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap();
                use std::io::Write;
                writeln!(f, "assistant: resumed").unwrap();
            }
        });

        assert!(verify_session_resumed(
            &path,
            "[permission]",
            baseline,
            Duration::from_secs(3)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verify_times_out_fail_soft() {
        let dir = std::env::temp_dir().join(format!("ho-verify-t-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.jsonl");
        std::fs::write(&path, "[permission] approve?\n").unwrap();
        let baseline = std::fs::metadata(&path).unwrap().modified().unwrap();

        let start = Instant::now();
        assert!(!verify_session_resumed(
            &path,
            "[permission]",
            baseline,
            Duration::from_millis(400)
        ));
        assert!(start.elapsed() >= Duration::from_millis(350));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
