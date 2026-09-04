//! Persisted per-agent session state (pinned sessions).
//!
//! A mode-0600 JSON sibling of `config.toml` — session ids only, never
//! transcript contents. Separate from the in-memory handoff history on
//! purpose: this survives restarts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::DaemonError;

/// Persisted per-agent session state. Currently: the user's pinned session
/// per agent. Written as JSON (mode 0600) next to config — never transcript
/// contents, only session ids.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// agent_id -> pinned session id (set via `handover attach` / API).
    pub pinned: HashMap<String, String>,
}

/// The session-state file path for a given config path: the sibling
/// `session_state.json` next to `config.toml`. Mirrors the production
/// `Config::session_state_path()` while staying next to whatever config path
/// is in use (so tests inject a temp one).
pub(crate) fn session_state_path_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("session_state.json")
}

/// Reads the persisted session state; missing/corrupt file → empty state.
pub(crate) fn load_session_state(path: &Path) -> SessionState {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            log::warn!("could not parse session state {}: {e}", path.display());
            SessionState::default()
        }),
        Err(_) => SessionState::default(),
    }
}

/// Writes the session-state file (mode 0600), atomically (temp + rename — a
/// crash mid-write never leaves a truncated state file). Best-effort: a
/// failure to persist a pin is logged, never fatal.
pub(crate) fn save_session_state(path: &Path, state: &SessionState) -> Result<(), DaemonError> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| DaemonError::Message(format!("session state serialize: {e}")))?;
    handover_config::atomic_write_private(path, raw.as_bytes())
        .map_err(|e| DaemonError::Message(format!("session state write: {e}")))
}
