//! Off-lock status computation: agent status lights and live sessions.
//!
//! [`compute_agents_status`] / [`compute_live_sessions`] take cheap
//! snapshots gathered under the daemon lock (see `lib.rs`) so probes,
//! subprocesses and filesystem walks can run without it — one slow agent
//! must never freeze the palette or the API.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use handover_core::agent::AgentMetaStatus;
use handover_core::approval::{tail_contains_marker, tail_marker_line};
use handover_core::session::{
    live_sessions_from_cli_rows, parse_cli_list_output, scan_live_sessions_home, LiveSession,
    SessionDiscovery, SessionSpec, DEFAULT_STALENESS,
};

use crate::process::agent_process_running;
use crate::provider_health::ProviderHealth;
use crate::runner::{resolve_cli_program, run_cli_list};

/// Cheap inputs for [`compute_agents_status`], gathered under the daemon
/// lock so the probes (process check, provider health) can run without it.
pub struct AgentsStatusSnapshot {
    /// Registry statuses (filesystem detection only — no subprocesses).
    pub statuses: Vec<AgentMetaStatus>,
    /// agent id → launch command, for the live-process check.
    pub commands: HashMap<String, String>,
    /// Shared provider-health probe state (internally synchronized).
    pub provider_health: Arc<ProviderHealth>,
}

/// Cheap inputs for [`compute_live_sessions`], gathered under the daemon
/// lock so session scanning (globs, cli-list subprocesses, tail peeks) can
/// run without it.
pub struct LiveSessionsSnapshot {
    /// Session-discovery specs (glob or cli-list), config + catalog.
    pub specs: Vec<SessionSpec>,
    /// Home dir the glob scanner resolves session globs against.
    pub home: PathBuf,
    /// agent id → permission marker, for session-aware agents only (the
    /// opt-in approval peek). Empty when no agent opted in.
    pub markers: HashMap<String, String>,
}

/// Computes agent statuses off the daemon lock: the live-process check
/// (`agent_process_running`) and the provider-health probe. Both are
/// fail-soft and read shared state only via the snapshot.
pub fn compute_agents_status(snapshot: AgentsStatusSnapshot) -> Vec<AgentMetaStatus> {
    let mut statuses = snapshot.statuses;
    // The availability probe tells us the binary is installed; "running"
    // is a live-process check so the status light reflects what the agent
    // is doing RIGHT NOW (Hermes with its TUI open is online; an installed
    // OpenCode with no process is not).
    for s in &mut statuses {
        if let Some(command) = snapshot.commands.get(&s.meta.id) {
            s.status.running = agent_process_running(command);
        }
        // Provider health: when the binary is fine but its model endpoint
        // is dead, say so BEFORE a send is wasted. Fail-soft everywhere.
        if s.status.available {
            if let Some(reason) = snapshot.provider_health.down_reason(&s.meta.id) {
                s.status.provider_down = Some(reason);
            }
        }
    }
    statuses
}

/// Computes the live-session listing off the daemon lock: glob scan,
/// cli-list subprocesses, and the opt-in approval tail-peek. A hung
/// cli-list fails soft (skip the agent) — this must never be run under
/// the daemon lock, or one slow agent would freeze the palette and API.
pub fn compute_live_sessions(snapshot: &LiveSessionsSnapshot) -> Vec<LiveSession> {
    let now = chrono::Utc::now();
    let mut sessions =
        scan_live_sessions_home(&snapshot.specs, now, DEFAULT_STALENESS, &snapshot.home);

    // Cli-list specs are skipped by the glob scanner — run them here.
    for spec in &snapshot.specs {
        let SessionDiscovery::CliList { command } = &spec.discovery else {
            continue;
        };
        let Some((program, args)) = command.split_first() else {
            continue;
        };
        // Finder-launched apps inherit a restricted PATH (no
        // `~/.local/bin`), so a bare cli-list name like `hermes` would
        // silently discover nothing. Resolve it explicitly first.
        let program = resolve_cli_program(program);
        // A hung listing must never stall a palette open or a send —
        // fail soft (skip the agent) instead of blocking indefinitely.
        let output = match run_cli_list(&program, args) {
            Some(o) => o,
            None => {
                log::debug!(
                    "session cli-list for `{}` failed or timed out",
                    spec.agent_id
                );
                continue;
            }
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let rows = parse_cli_list_output(&text);
        sessions.extend(live_sessions_from_cli_rows(
            &spec.agent_id,
            rows,
            now,
            DEFAULT_STALENESS,
        ));
    }

    // Approval peek — the ONE documented privacy exception, opt-in per
    // agent: only sessions of agents with a `permission_marker` are ever
    // read, and only the last few KB (see the approval module). Agents
    // without a marker are never peeked at (`blocked` stays None).
    for s in &mut sessions {
        if let Some(path) = &s.path {
            if let Some(marker) = snapshot.markers.get(&s.agent_id) {
                let blocked = tail_contains_marker(path, marker);
                s.blocked = Some(blocked);
                // Only a blocked session surfaces WHAT is being asked
                // (the marker line) — the approval card shows it.
                if blocked {
                    s.blocked_detail = tail_marker_line(path, marker);
                }
            }
        }
    }

    sessions
}
