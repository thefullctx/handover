use crate::action::Action;
use crate::capture::Capture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Availability probe result for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub available: bool,
    pub detail: String,
    /// True when the agent's process is alive right now (filled by the
    /// daemon's live-process check, not by `detect()`). The status light's
    /// "online" cue — installed-but-idle reads as offline.
    #[serde(default)]
    pub running: bool,
    /// Set when the agent IS available but its model provider is currently
    /// unreachable — the daemon sniffed the endpoint from the agent's config
    /// and a cheap probe could not connect. A send would fail with a
    /// connection error; the UI can say so BEFORE the user wastes one.
    /// `None` = no opinion (endpoint unknown, or probe passed).
    #[serde(default)]
    pub provider_down: Option<String>,
}

/// A target live session for a session-aware handoff (resume-by-id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTarget {
    /// Agent whose session this is (matches the resolved agent).
    pub agent_id: String,
    /// The session id as it appears in the resume flag (e.g. `019ffd73-…`,
    /// `20260812_130220_dbf5cf`).
    pub session_id: String,
    /// True when the user explicitly chose this session (`--session`, a
    /// pinned session, a chat follow-up) — never silently abandoned. False
    /// when the daemon picked it heuristically (freshest-first): a resume
    /// refusal there (e.g. another live process owns it) may fall back to a
    /// fresh send instead of failing the handoff.
    #[serde(default)]
    pub explicit: bool,
}

/// A request to hand a capture + rendered prompt to an agent.
#[derive(Debug, Clone)]
pub struct AgentRequest {
    pub capture: Capture,
    pub action: Action,
    pub prompt: String,
    /// When set, the agent should resume this live session instead of
    /// spawning a fresh one. `None` = today's fresh-send behavior exactly.
    pub session: Option<SessionTarget>,
}

/// Which output pipe a streamed chunk came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputChannel {
    Stdout,
    Stderr,
}

/// Optional incremental-output sink. When provided, the agent calls it with
/// each chunk of stdout/stderr as it arrives (from a background drain thread)
/// so the GUI can render live progress. Must be cheap and never block.
pub type OutputSink = Arc<dyn Fn(OutputChannel, &str) + Send + Sync>;

/// A structured confirmation that an agent received (or failed) a handoff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendReceipt {
    pub ok: bool,
    pub agent_id: String,
    pub agent_name: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    pub duration_ms: u64,
    /// The session the reply landed in, when known: the requested session id,
    /// or a NEW id when the agent rotated sessions on resume (Phase 0 fact 4).
    /// Lets the tracker update which session is current. `None` for fresh sends.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// A resume refusal because another live process owns the session (Hermes:
/// "Session <id> already has a live owner (cli, pid 89301, running 1m) —
/// only 1 live connection per session"). The listing never shows ownership,
/// so this is only observable by attempting the resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveOwnerRefusal {
    /// The session that refused the second connection.
    pub session_id: String,
    /// Who holds it, e.g. "cli, pid 89301" (parenthesized group minus the
    /// running-age). `None` when the output names no owner.
    pub owner: Option<String>,
}

/// Parses a live-owner refusal out of a failed resume's output (stderr first,
/// then stdout). Returns `None` for any other failure — callers use it to
/// decide between "retry fresh" (auto-resolved sessions) and a plain,
/// actionable error (explicit sessions). Never panics on odd output.
pub fn parse_live_owner_refusal(
    stderr: Option<&str>,
    stdout: Option<&str>,
) -> Option<LiveOwnerRefusal> {
    const MARKER: &str = "already has a live owner";
    let combined = format!(
        "{}\n{}",
        stderr.unwrap_or_default(),
        stdout.unwrap_or_default()
    );
    let idx = combined.find(MARKER)?;
    // Session id: the token right after "Session " ahead of the marker.
    let before = combined[..idx].trim_end();
    let session_id = before
        .split_whitespace()
        .rev()
        .find(|t| *t != "Session")
        .unwrap_or("?")
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
        .to_string();
    // Owner: the parenthesized group after the marker, minus running-age.
    let after = &combined[idx + MARKER.len()..];
    let owner = after.find('(').and_then(|s| {
        after[s + 1..].find(')').and_then(|e| {
            let inner = after[s + 1..s + 1 + e].trim();
            if inner.is_empty() {
                return None;
            }
            let kept: Vec<&str> = inner
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty() && !p.starts_with("running"))
                .take(2)
                .collect();
            if kept.is_empty() {
                None
            } else {
                Some(kept.join(", "))
            }
        })
    });
    Some(LiveOwnerRefusal { session_id, owner })
}

/// Serializable description of an agent for listings and the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    /// e.g. "command" | "openai_compatible" | "hermes" | ...
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_summary: Option<String>,
    /// Which agent version this adapter's CLI assumptions were verified
    /// against, e.g. `"adapter: 0.160.0 (verified 2026-10-04)"`. Present only
    /// for agents Handover ships an adapter for (see the config crate's
    /// `adapter` module). A bug report can then be weighed against the agent
    /// version the user actually has — the manual substitute for probing
    /// every agent's `--version` on every status poll.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<String>,
    #[serde(default)]
    pub demo: bool,
}

/// An agent paired with its live availability status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetaStatus {
    pub meta: AgentMeta,
    pub status: AgentStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("{0}")]
    Message(String),
    #[error("Agent `{agent}` is not available. {detail}")]
    Unavailable { agent: String, detail: String },
    #[error("Agent `{agent}` failed. {detail}")]
    Failed { agent: String, detail: String },
    #[error("Agent `{agent}` did not respond within {secs}s")]
    Timeout { agent: String, secs: u64 },
}

impl From<std::io::Error> for AgentError {
    fn from(e: std::io::Error) -> Self {
        AgentError::Message(format!("I/O error: {e}"))
    }
}

/// Every agent integration implements this. The UI only ever talks to these
/// abstractions — it is never coupled to a specific agent.
pub trait Agent: Send + Sync {
    fn meta(&self) -> AgentMeta;
    fn detect(&self) -> Result<AgentStatus, AgentError>;
    /// Executes a handoff. `stream`, when `Some`, receives incremental
    /// stdout/stderr chunks as they arrive (see [`OutputSink`]); pass `None`
    /// to run without streaming.
    fn send(
        &self,
        request: &AgentRequest,
        stream: Option<OutputSink>,
    ) -> Result<SendReceipt, AgentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hermes_live_owner_refusal() {
        let stderr = "Session 20260903_103656_c0f480 already has a live owner \
            (cli, pid 89301, running 1m). Only 1 service at a time may run a session.";
        let r = parse_live_owner_refusal(Some(stderr), None).expect("refusal");
        assert_eq!(r.session_id, "20260903_103656_c0f480");
        assert_eq!(r.owner.as_deref(), Some("cli, pid 89301"));
    }

    #[test]
    fn refusal_found_on_stdout_when_stderr_empty() {
        let out = "Session abc123 already has a live owner (tui, pid 7, running 2h).";
        let r = parse_live_owner_refusal(None, Some(out)).expect("refusal");
        assert_eq!(r.session_id, "abc123");
        assert_eq!(r.owner.as_deref(), Some("tui, pid 7"));
    }

    #[test]
    fn ordinary_failures_are_not_refusals() {
        assert_eq!(
            parse_live_owner_refusal(Some("boom"), Some("Exited 1")),
            None
        );
        assert_eq!(parse_live_owner_refusal(None, None), None);
    }
}
