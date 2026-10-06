//! The session-aware agent: resumes into the user's live sessions by id.
//!
//! Session-aware handoff reduces to "resume by id": when a request carries a
//! [`SessionTarget`], the agent runs its configured `resume_command` with
//! `{SESSION}` substituted instead of the plain fresh-send command. The reply
//! still comes back as streamed stdout/stderr, so the result panel, live
//! progress, history and retry all keep working — only the command changes.
//!
//! Without a session target the agent behaves *identically* to the generic
//! command agent (fresh send). Agents without a `resume_command` are never
//! affected.

use handover_config::AgentConfig;
use handover_core::agent::{
    parse_live_owner_refusal, Agent, AgentError, AgentMeta, AgentRequest, AgentStatus, OutputSink,
    SendReceipt,
};

use crate::generic_command::{run_command, GenericCommandAgent};

/// Session-aware agent. `meta`/`detect` delegate to the generic command agent
/// (same binary, same availability); `send` chooses the resume command when a
/// live session is targeted.
pub struct SessionAgent {
    config: AgentConfig,
    inner: GenericCommandAgent,
}

impl SessionAgent {
    pub fn new(config: AgentConfig) -> Self {
        let inner = GenericCommandAgent::new(config.clone());
        Self { config, inner }
    }
}

impl Agent for SessionAgent {
    fn meta(&self) -> AgentMeta {
        let mut meta = self.inner.meta();
        meta.kind = "session".to_string();
        // Surface the resume form (not just the fresh command) so the UI can
        // explain what "send into session" will run.
        meta.config_summary = self
            .config
            .resume_command
            .clone()
            .or_else(|| Some(self.config.command.clone()));
        meta
    }

    fn detect(&self) -> Result<AgentStatus, AgentError> {
        self.inner.detect()
    }

    fn send(
        &self,
        request: &AgentRequest,
        stream: Option<OutputSink>,
    ) -> Result<SendReceipt, AgentError> {
        let target = request.session.as_ref();
        // With a live session + a resume command → resume by id. Anything
        // else (no session, or an agent without resume support) → fresh send,
        // byte-for-byte identical to the generic agent today.
        let (command, session_opt) = match (target, self.config.resume_command.as_deref()) {
            (Some(target), Some(resume)) if !resume.trim().is_empty() => {
                (resume.trim(), Some(target.session_id.as_str()))
            }
            _ => (self.config.command.trim(), None),
        };

        let mut receipt = run_command(
            &self.config,
            command,
            &request.prompt,
            session_opt,
            stream.clone(),
        )?;

        // Live-owner fallback: some agents refuse headless resume into a
        // session another live process owns (Hermes: "already has a live
        // owner" — e.g. its TUI is open there). The listing never shows
        // ownership, so this is only observable by trying. When the session
        // was picked heuristically (freshest-first — the user expressed no
        // attachment to it), retry once as a fresh send so the handoff still
        // gets done; the detail says where the reply landed. Explicit
        // targets (CLI --session, pins, chat follow-ups) never fall back
        // silently — the refusal comes back as the failure instead, and the
        // daemon humanizes it into an actionable error.
        let auto = target.map(|t| !t.explicit).unwrap_or(false);
        if !receipt.ok && session_opt.is_some() && auto {
            let refusal =
                parse_live_owner_refusal(receipt.stderr.as_deref(), receipt.stdout.as_deref());
            if let Some(refusal) = refusal {
                let mut fresh = run_command(
                    &self.config,
                    self.config.command.trim(),
                    &request.prompt,
                    None,
                    stream,
                )?;
                // The fresh run mints a new session: adopt its id from the
                // agent's own footer when present so follow-ups resume the
                // session that actually has the context (else None, and
                // follow-ups re-resolve freshest — usually this same run).
                fresh.session_id = fresh
                    .stdout
                    .as_deref()
                    .and_then(|out| extract_new_session_id(&self.config.id, out, ""));
                let owner = refusal.owner.map(|o| format!(" ({o})")).unwrap_or_default();
                fresh.detail = format!(
                    "Live session {} is already open elsewhere{} — started a fresh session instead.",
                    refusal.session_id, owner
                );
                return Ok(fresh);
            }
        }

        // Phase 0 fact 4: a resume may ROTATE to a new session id. Best-effort
        // parse it from stdout so the tracker can update which session is now
        // current. When nothing new is found, the reply landed in the session
        // we asked for — record that so the tracker still knows.
        if let Some(target) = target {
            receipt.session_id = Some(
                receipt
                    .stdout
                    .as_deref()
                    .and_then(|out| {
                        extract_new_session_id(&self.config.id, out, &target.session_id)
                    })
                    .unwrap_or_else(|| target.session_id.clone()),
            );
        }
        Ok(receipt)
    }
}

/// Best-effort extraction of a NEW session id from a resume's stdout.
///
/// Agents differ in whether (and how) they report a rotated session id:
/// * Hermes prints `hermes --resume <id>` / `--continue` hints on exit;
///   ids look like `20260812_130220_dbf5cf`.
/// * claude/codex/droid/omp use uuid ids (`019ffd73-…`).
///
/// Strategy: scan stdout for tokens matching the agent's known id shape on
/// lines that mention a resume hint (`session`/`resume`/`--continue`), and
/// never return the id we already targeted (echoed ids are not rotations).
/// Returns `None` when nothing convincing is found — the caller then keeps
/// the requested session id.
///
/// An id-shaped token on an ordinary output line is NOT trusted: the whole
/// point is detecting a *rotation*, and a uuid in some log line would send
/// the tracker to a session that does not exist.
fn extract_new_session_id(agent_id: &str, stdout: &str, requested: &str) -> Option<String> {
    let mut hinted: Option<String> = None;

    for line in stdout.lines() {
        let lower = line.to_ascii_lowercase();
        let is_hint = lower.contains("session")
            || lower.contains("resume")
            || lower.contains("--continue")
            || lower.contains("-r ");
        if !is_hint {
            continue;
        }
        for token in line.split_whitespace() {
            let cleaned =
                token.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
            if cleaned == requested || cleaned.is_empty() {
                continue;
            }
            if matches_id_shape(agent_id, cleaned) {
                hinted = Some(cleaned.to_string());
            }
        }
    }

    hinted
}

type IdShape = fn(&str) -> bool;

/// Agents whose session ids are not UUIDs, with the shape check for each.
/// Every listed agent must have an `ADAPTER_COMPAT` declaration (a test
/// enforces it), so a new id shape cannot be added without one.
const ID_SHAPES: &[(&str, IdShape)] = &[("hermes", is_hermes_id)];

/// True when a token looks like this agent's session id.
fn matches_id_shape(agent_id: &str, token: &str) -> bool {
    match ID_SHAPES.iter().find(|(id, _)| *id == agent_id) {
        Some((_, is_shape)) => is_shape(token),
        None => is_uuid(token),
    }
}

/// `20260812_130220_dbf5cf` (8 digits, 6 digits, 6 hex).
fn is_hermes_id(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() == 22
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'_'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'_'
        && b[16..].iter().all(|c| c.is_ascii_hexdigit())
}

/// 8-4-4-4-12 hex, e.g. `019ffd73-95b5-7240-b6bb-a39b1952730b`.
fn is_uuid(token: &str) -> bool {
    let b = token.as_bytes();
    let ok = |range: std::ops::Range<usize>| b[range].iter().all(|c| c.is_ascii_hexdigit());
    b.len() == 36
        && ok(0..8)
        && b[8] == b'-'
        && ok(9..13)
        && b[13] == b'-'
        && ok(14..18)
        && b[18] == b'-'
        && ok(19..23)
        && b[23] == b'-'
        && ok(24..36)
}

#[cfg(test)]
mod tests {
    use super::*;
    use handover_core::agent::{AgentRequest, SessionTarget};
    use handover_core::capture::{Capture, SourceKind};
    use std::collections::HashMap;

    #[test]
    fn every_id_shape_agent_declares_adapter_compat() {
        // Fourth drift surface: a non-UUID id shape is an assumption about one
        // agent's CLI, so it needs an ADAPTER_COMPAT declaration like any other.
        for (agent_id, _) in ID_SHAPES {
            assert!(
                handover_config::compat_for(agent_id).is_some(),
                "{agent_id}: has a session-id shape but no ADAPTER_COMPAT entry"
            );
        }
    }

    fn config_with(command: &str, resume: Option<&str>, session_glob: Option<&str>) -> AgentConfig {
        AgentConfig {
            id: "test-session".into(),
            name: "Test Session Agent".into(),
            kind: handover_config::AgentKind::Session,
            command: command.into(),
            description: None,
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: Some(30),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: session_glob.map(|s| s.to_string()),
            session_cli_list: None,
            resume_command: resume.map(|s| s.to_string()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        }
    }

    fn request_with_session(prompt: &str, session_id: &str) -> AgentRequest {
        AgentRequest {
            capture: Capture::text(SourceKind::Manual, "the error", None),
            action: handover_core::action::builtin_actions().remove(0),
            prompt: prompt.to_string(),
            session: Some(SessionTarget {
                agent_id: "test-session".into(),
                session_id: session_id.into(),
                explicit: true,
            }),
        }
    }

    fn request_with_auto_session(prompt: &str, session_id: &str) -> AgentRequest {
        let mut req = request_with_session(prompt, session_id);
        if let Some(t) = req.session.as_mut() {
            t.explicit = false;
        }
        req
    }

    fn request_fresh(prompt: &str) -> AgentRequest {
        AgentRequest {
            capture: Capture::text(SourceKind::Manual, "the error", None),
            action: handover_core::action::builtin_actions().remove(0),
            prompt: prompt.to_string(),
            session: None,
        }
    }

    #[test]
    fn resumes_with_session_placeholder_substituted() {
        // printf echoes the prompt and the session flag so we can assert the
        // resume command was chosen and {SESSION} substituted.
        let agent = SessionAgent::new(config_with(
            "printf 'FRESH:%s' \"{PROMPT}\"",
            Some("printf 'RESUME:%s SID=%s' \"{PROMPT}\" \"{SESSION}\""),
            Some("~/.x/*.jsonl"),
        ));
        let receipt = agent
            .send(&request_with_session("hello session", "abc123"), None)
            .unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        let out = receipt.stdout.as_deref().unwrap_or("");
        assert!(
            out.contains("RESUME:hello session") && out.contains("SID=abc123"),
            "expected resume command with substituted session: {out}"
        );
        // The session we asked for is recorded on the receipt.
        assert_eq!(receipt.session_id.as_deref(), Some("abc123"));
    }

    #[test]
    fn fresh_send_uses_plain_command() {
        let agent = SessionAgent::new(config_with(
            "printf 'FRESH:%s' \"{PROMPT}\"",
            Some("printf 'RESUME:%s' \"{PROMPT}\""),
            Some("~/.x/*.jsonl"),
        ));
        let receipt = agent.send(&request_fresh("plain prompt"), None).unwrap();
        let out = receipt.stdout.as_deref().unwrap_or("");
        assert!(
            out.contains("FRESH:plain prompt"),
            "fresh send must use the plain command: {out}"
        );
        assert!(receipt.session_id.is_none(), "fresh send has no session");
    }

    #[test]
    fn agent_without_resume_command_always_sends_fresh() {
        // No resume_command → a session target is ignored (fallback to today).
        let agent = SessionAgent::new(config_with("printf 'PLAIN:%s' \"{PROMPT}\"", None, None));
        let receipt = agent
            .send(&request_with_session("ignored", "abc123"), None)
            .unwrap();
        let out = receipt.stdout.as_deref().unwrap_or("");
        assert!(out.contains("PLAIN:ignored"), "got: {out}");
    }

    #[test]
    fn meta_and_detect_delegate_to_generic() {
        let agent = SessionAgent::new(config_with(
            "cat",
            Some("cat {SESSION}"),
            Some("~/.x/*.jsonl"),
        ));
        let meta = agent.meta();
        assert_eq!(meta.kind, "session");
        assert_eq!(meta.config_summary.as_deref(), Some("cat {SESSION}"));
        assert!(agent.detect().unwrap().available, "cat should be found");
    }

    #[test]
    fn streams_output_chunks_in_order() {
        use std::sync::{Arc, Mutex};
        let agent = SessionAgent::new(config_with(
            "printf 'one\\n'; sleep 0.05; printf 'two\\n'",
            Some("printf 'one\\n'; sleep 0.05; printf 'two\\n'"),
            Some("~/.x/*.jsonl"),
        ));
        let chunks = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink: OutputSink = {
            let chunks = Arc::clone(&chunks);
            Arc::new(move |_channel, chunk| chunks.lock().unwrap().push(chunk.to_string()))
        };
        let receipt = agent.send(&request_fresh("x"), Some(sink)).unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        let all = chunks.lock().unwrap().join("");
        assert!(all.contains("one") && all.contains("two"));
        assert!(all.find("one").unwrap() < all.find("two").unwrap());
    }

    #[test]
    fn times_out_on_hung_command() {
        // timeout_secs = 30 would make this slow; build one with a 1s timeout.
        let mut config = config_with("sleep 5", Some("sleep 5"), Some("~/.x/*.jsonl"));
        config.timeout_secs = Some(1);
        let agent = SessionAgent::new(config);
        match agent.send(&request_fresh("x"), None) {
            Err(AgentError::Timeout { secs, .. }) => assert_eq!(secs, 1),
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    // -- rotated-id parsing ------------------------------------------------

    #[test]
    fn detects_rotated_hermes_id_from_resume_hint() {
        let out = "Resuming session 20260812_034825_a2a3e8\n\
                   hermes --resume 20260812_130220_dbf5cf  # next time\n";
        assert_eq!(
            extract_new_session_id("hermes", out, "20260812_034825_a2a3e8").as_deref(),
            Some("20260812_130220_dbf5cf")
        );
    }

    #[test]
    fn detects_rotated_uuid_id() {
        let out = "resume session 019ffd73-95b5-7240-b6bb-a39b1952730b\n";
        assert_eq!(
            extract_new_session_id("codex", out, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").as_deref(),
            Some("019ffd73-95b5-7240-b6bb-a39b1952730b")
        );
    }

    #[test]
    fn echoed_requested_id_is_not_a_rotation() {
        // Stdout merely echoes the session we asked for → no new id.
        let out = "Now in session abc123\n";
        assert_eq!(extract_new_session_id("claude", out, "abc123"), None);
    }

    #[test]
    fn no_plausible_token_returns_none() {
        let out = "done. nothing else here.\n";
        assert_eq!(extract_new_session_id("hermes", out, "abc"), None);
        // An id-shaped token on an ordinary (non-hint) line is NOT trusted —
        // it could be a uuid from a log line, not a rotation.
        let out = "some tool output 019ffd73-95b5-7240-b6bb-a39b1952730b end\n";
        assert_eq!(extract_new_session_id("codex", out, "abc"), None);
    }

    // -- live-owner fallback ------------------------------------------------

    fn owner_refusal_resume() -> &'static str {
        "echo 'Session {SESSION} already has a live owner (cli, pid 42, running 1m)' >&2; exit 1"
    }

    #[test]
    fn auto_session_falls_back_to_fresh_on_live_owner_refusal() {
        let agent = SessionAgent::new(config_with(
            "printf 'FRESH-OK'",
            Some(owner_refusal_resume()),
            Some("~/.x/*.jsonl"),
        ));
        let receipt = agent
            .send(&request_with_auto_session("do work", "owned-1"), None)
            .unwrap();
        assert!(
            receipt.ok,
            "fallback fresh send must succeed: {}",
            receipt.detail
        );
        assert!(
            receipt.detail.contains("owned-1") && receipt.detail.contains("fresh session instead"),
            "detail must name the owned session and the fallback: {}",
            receipt.detail
        );
        assert_eq!(receipt.stdout.as_deref(), Some("FRESH-OK"));
        assert_eq!(receipt.session_id, None, "no id in fresh output");
    }

    #[test]
    fn explicit_session_never_falls_back_on_live_owner_refusal() {
        let agent = SessionAgent::new(config_with(
            "printf 'FRESH-OK'",
            Some(owner_refusal_resume()),
            Some("~/.x/*.jsonl"),
        ));
        let receipt = agent
            .send(&request_with_session("do work", "owned-9"), None)
            .unwrap();
        assert!(!receipt.ok, "explicit refusal must stay a failure");
        assert!(
            receipt.detail.contains("Exited with status 1"),
            "detail: {}",
            receipt.detail
        );
        assert_eq!(receipt.session_id.as_deref(), Some("owned-9"));
        assert!(
            receipt.stdout.as_deref().unwrap_or_default().is_empty(),
            "fresh command must not have run"
        );
    }

    #[test]
    fn auto_session_does_not_fall_back_on_ordinary_failures() {
        let agent = SessionAgent::new(config_with(
            "printf 'SHOULD-NOT-RUN'",
            Some("echo 'plain boom' >&2; exit 1"),
            Some("~/.x/*.jsonl"),
        ));
        let receipt = agent
            .send(&request_with_auto_session("do work", "s-1"), None)
            .unwrap();
        assert!(!receipt.ok);
        assert!(
            receipt.stdout.as_deref().unwrap_or_default().is_empty(),
            "fresh command must not have run on non-owner failures"
        );
    }

    #[test]
    fn fallback_adopts_fresh_session_id_from_footer() {
        let agent = SessionAgent::new(config_with(
            "printf 'answer\\nresuming session 019ffd73-95b5-7240-b6bb-a39b1952730b next\\n'",
            Some(owner_refusal_resume()),
            Some("~/.x/*.jsonl"),
        ));
        let receipt = agent
            .send(&request_with_auto_session("do work", "owned-1"), None)
            .unwrap();
        assert!(receipt.ok, "{}", receipt.detail);
        assert_eq!(
            receipt.session_id.as_deref(),
            Some("019ffd73-95b5-7240-b6bb-a39b1952730b")
        );
    }
}
