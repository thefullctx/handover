//! Handover core — the cross-platform domain model.
//!
//! This crate contains the standardized `Capture` object, `Action`
//! definitions, the `Agent` trait, prompt rendering and security
//! exclusions. It is intentionally free of any GUI, daemon or platform
//! specific code so it can be shared by the daemon, the GUI and the CLI.

pub mod action;
pub mod agent;
pub mod approval;
pub mod capture;
pub mod exclusions;
pub mod prompt;
pub mod session;

pub use action::{builtin_actions, find_action, Action};
pub use agent::{
    Agent, AgentError, AgentMeta, AgentMetaStatus, AgentRequest, AgentStatus, SendReceipt,
    SessionTarget,
};
pub use approval::{tail_contains_marker, APPROVAL_PEEK_BYTES};
pub use capture::{Capture, CaptureContent, CaptureSource, ContentKind, SourceKind};
pub use exclusions::{default_excluded_paths, is_excluded_path, matches_glob, matches_glob_ci};
pub use prompt::{render_action_prompt, render_capture_context};
pub use session::{
    activity_state, freshest_per_agent, hermes_id_timestamp, is_stale, live_sessions_from_cli_rows,
    parse_cli_list_output, scan_live_sessions, session_id_from_filename, ActivityState, CliListRow,
    LiveSession, SessionDiscovery, SessionSpec, DEFAULT_STALENESS, SESSION_ID_RULE_AGENTS,
    WORKING_WINDOW,
};
