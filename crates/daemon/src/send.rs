//! The send pipeline's slow (lock-free) half plus execution.
//!
//! The daemon lock covers only the cheap plan half (see `lib.rs`); probes,
//! prompt rendering, session resolution and the agent subprocess itself
//! happen here without the lock.

use std::sync::Arc;

use handover_core::action::Action;
use handover_core::agent::{AgentError, AgentRequest, SessionTarget};
use handover_core::capture::Capture;
use handover_core::prompt::{render_action_prompt, render_capture_context};
use handover_core::session::LiveSession;

use crate::history::SendOutcome;
use crate::provider_health::ProviderHealth;
use crate::status::{compute_live_sessions, LiveSessionsSnapshot};
use crate::DaemonError;

/// Fast (lock-held) half of a handoff: normalized capture, resolved action
/// and agent, and the snapshots needed to finish the job off the lock.
/// [`complete_send`] does the slow half — probes, prompt render, session
/// resolution, request assembly.
pub struct SendPlan {
    pub agent_id: String,
    pub agent: Arc<dyn handover_core::agent::Agent>,
    pub action: Action,
    pub capture: Capture,
    pub explicit_session: Option<String>,
    pub pinned_session: Option<String>,
    pub snapshot: LiveSessionsSnapshot,
    pub provider_health: Arc<ProviderHealth>,
}

/// Slow half of a handoff, off the daemon lock: provider-health probe,
/// prompt render, and session resolution. Returns the agent + prepared
/// request so the caller never holds the lock across a subprocess or
/// filesystem walk.
pub fn complete_send(
    plan: SendPlan,
) -> Result<(Arc<dyn handover_core::agent::Agent>, AgentRequest), DaemonError> {
    // Resolve the session BEFORE the provider gate: the gate must probe the
    // endpoint THIS send will use (the target session's provider for
    // resumes, the ambient default for fresh sends), not the freshest
    // session's. Fail fast when it is unreachable: a doomed run would
    // otherwise hang for the full timeout before dying with an opaque
    // connection error. Sub-second TCP probe, fail-open on unknowns.
    let live: Vec<LiveSession> = compute_live_sessions(&plan.snapshot)
        .into_iter()
        .filter(|s| s.agent_id == plan.agent_id)
        .collect();
    let session = resolve_session_for_agent(
        &plan.agent_id,
        plan.explicit_session.as_deref(),
        plan.pinned_session.as_deref(),
        &live,
    );
    if let Some(reason) = plan.provider_health.send_readiness(
        &plan.agent_id,
        session.as_ref().map(|s| s.session_id.as_str()),
    ) {
        return Err(DaemonError::Message(format!(
            "Cannot hand off to {agent_name}: {reason}. Start the provider (or switch the agent's model) and try again.",
            agent_name = plan.agent.meta().name,
        )));
    }

    let prompt = if plan.capture.metadata.get("chat").map(String::as_str) == Some("true")
        && plan.action.id == "ask"
    {
        render_capture_context(&plan.capture)
    } else {
        render_action_prompt(&plan.action, &plan.capture)
    };

    let request = AgentRequest {
        capture: plan.capture,
        action: plan.action,
        prompt,
        session,
    };

    Ok((plan.agent, request))
}

/// Session resolution for one agent (section 6 order):
/// 1. explicitly passed session id,
/// 2. the persisted pinned session for that agent (if still live),
/// 3. the freshest live session for that agent,
///
/// otherwise `None` → fresh send.
fn resolve_session_for_agent(
    agent_id: &str,
    explicit: Option<&str>,
    pinned: Option<&str>,
    live: &[LiveSession],
) -> Option<SessionTarget> {
    if let Some(sid) = explicit {
        if !sid.trim().is_empty() {
            return Some(SessionTarget {
                agent_id: agent_id.to_string(),
                session_id: sid.trim().to_string(),
                explicit: true,
            });
        }
    }
    if let Some(sid) = pinned {
        if live.iter().any(|s| s.session_id == sid) {
            return Some(SessionTarget {
                agent_id: agent_id.to_string(),
                session_id: sid.to_string(),
                explicit: true,
            });
        }
    }
    live.first().map(|s| SessionTarget {
        agent_id: agent_id.to_string(),
        session_id: s.session_id.clone(),
        explicit: false,
    })
}

/// Slow half of a handoff, run *without* the daemon lock: executes the agent
/// and assembles the outcome. Keeps long-running agents from blocking the
/// palette hotkey, `/health`, and concurrent requests. `stream` forwards live
/// output chunks when `Some` (the GUI); the CLI/HTTP path passes `None`.
pub fn execute_handoff(
    agent: Arc<dyn handover_core::agent::Agent>,
    request: AgentRequest,
    stream: Option<handover_core::agent::OutputSink>,
    id: String,
) -> SendOutcome {
    // Keep a copy of the capture so history rows can retry / duplicate / send
    // the same context to a different agent.
    let capture = request.capture.clone();
    let (receipt, mut error) = match agent.send(&request, stream) {
        Ok(receipt) => (Some(receipt), None),
        Err(e) => (None, Some(humanize_agent_error(&e))),
    };
    let ok = receipt.as_ref().map(|r| r.ok).unwrap_or(false);
    // A resume refused for a live-owned session is not an agent crash: say
    // what happened and what to do. Explicit targets never fall back
    // silently, so this message is the only voice that failure gets.
    if error.is_none() {
        if let Some(r) = receipt.as_ref() {
            if !r.ok {
                error = humanize_receipt_failure(&agent.meta().name, r);
            }
        }
    }
    // Session id for the outcome: prefer what the AGENT reported. A resume
    // may rotate to a new session id (Phase 0 fact 4) — the adapter records
    // the rotated (or confirmed) id on the receipt, and losing it would send
    // follow-ups into a session the agent already abandoned.
    let session_id = receipt
        .as_ref()
        .and_then(|r| r.session_id.clone())
        .or_else(|| request.session.as_ref().map(|s| s.session_id.clone()));
    SendOutcome {
        id,
        ok,
        agent_id: agent.meta().id,
        agent_name: agent.meta().name,
        action_id: request.action.id.clone(),
        capture: Some(capture),
        created_at: chrono::Utc::now(),
        prompt: request.prompt,
        receipt,
        error,
        session_id,
    }
}

/// Converts agent errors into messages normal developers can act on —
/// never raw stack traces.
pub fn humanize_agent_error(e: &AgentError) -> String {
    match e {
        AgentError::Unavailable { agent, detail } => {
            format!("Agent `{agent}` is not available. {detail}")
        }
        AgentError::Failed { agent, detail } => {
            format!("Agent `{agent}` failed to receive the request. {detail}")
        }
        AgentError::Timeout { agent, secs } => {
            format!("Agent `{agent}` did not respond within {secs}s.")
        }
        AgentError::Message(m) => m.clone(),
    }
}

/// Plain-language error for known agent failure receipts. A resume refused
/// because another live process owns the session (explicit targets never
/// fall back) must not surface as raw agent prose or a bare "try again".
/// Returns `None` for unrecognized failures — those keep the receipt detail.
pub(crate) fn humanize_receipt_failure(
    agent_name: &str,
    receipt: &handover_core::agent::SendReceipt,
) -> Option<String> {
    let refusal = handover_core::agent::parse_live_owner_refusal(
        receipt.stderr.as_deref(),
        receipt.stdout.as_deref(),
    )?;
    let owner = refusal.owner.map(|o| format!(" ({o})")).unwrap_or_default();
    Some(format!(
        "{agent_name} session {} is already open elsewhere{}: two live connections \
         can't share one session. Close it there and retry, or start a new chat \
         to use a fresh session.",
        refusal.session_id, owner
    ))
}
