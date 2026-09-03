//! Session-only handoff history and stable handoff ids.
//!
//! `HandoffHistory` is deliberately not persisted (it dies with the
//! process) and lives behind its own lock so recording a completed handoff
//! never requires re-acquiring the daemon lock.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use handover_core::agent::SendReceipt;
use handover_core::capture::Capture;
use serde::Serialize;

/// Monotonic counter for stable, session-unique handoff ids (used by tray
/// rows and the history view to identify a specific outcome even after the
/// history has shifted).
static HANDOFF_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Result of a handoff, ready for serialization to the UI or CLI.
#[derive(Debug, Clone, Serialize)]
pub struct SendOutcome {
    /// Stable, session-unique id (e.g. `handoff-3`). Tray rows and history
    /// rows reference outcomes by this, never by position.
    pub id: String,
    pub ok: bool,
    pub agent_id: String,
    pub agent_name: String,
    /// The action that drove the prompt (e.g. `fix`, `ask`) — shown in the
    /// palette result panel and the tray history.
    pub action_id: String,
    /// The capture that was handed off — kept so history can offer retry /
    /// duplicate / send-to-another-agent without re-capturing.
    pub capture: Option<Capture>,
    /// When the handoff completed (UTC) — used by the history view to group
    /// entries by day (Today / Yesterday).
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub prompt: String,
    pub receipt: Option<SendReceipt>,
    pub error: Option<String>,
    /// The live session this handoff resumed into, when session-aware.
    /// `None` = fresh send (today's flow). Lets the UI show which path ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// How many completed handoffs are kept in the in-memory history.
pub const HANDOFF_HISTORY_CAP: usize = 10;

/// Session-only record of recent handoffs (newest first, capped).
///
/// Lives behind its own lock so a completed handoff can be recorded *after*
/// execution without re-acquiring the daemon lock (the fast half runs under
/// the daemon lock; the slow half must not). Shared by the palette, the tray
/// menu and the HTTP API. Not persisted — dies with the process.
#[derive(Clone, Default)]
pub struct HandoffHistory(Arc<Mutex<VecDeque<SendOutcome>>>);

impl HandoffHistory {
    /// Records a completed handoff, keeping the most recent `HANDOFF_HISTORY_CAP`.
    pub fn record(&self, outcome: SendOutcome) {
        let mut queue = self.0.lock().unwrap();
        queue.push_front(outcome);
        while queue.len() > HANDOFF_HISTORY_CAP {
            queue.pop_back();
        }
    }

    /// The recent handoffs, newest first.
    pub fn recent(&self) -> Vec<SendOutcome> {
        self.0.lock().unwrap().iter().cloned().collect()
    }

    /// Number of recorded handoffs (never exceeds `HANDOFF_HISTORY_CAP`).
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Empties the session history (the palette's "Clear history" action).
    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// Allocates the next stable, session-unique handoff id (e.g. `handoff-3`).
/// Generated *before* execution so streamed output events can carry the same
/// id as the eventual [`SendOutcome`].
pub fn next_handoff_id() -> String {
    format!(
        "handoff-{}",
        HANDOFF_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
