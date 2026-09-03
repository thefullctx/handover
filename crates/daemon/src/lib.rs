//! Handover daemon service.
//!
//! The daemon owns the configuration and the agent registry, captures
//! clipboard content, renders prompts and executes handoffs. It exposes the
//! same logic through two clients:
//! * the local HTTP API (used by the CLI, and served in-process by the GUI)
//! * direct in-process calls (used by the Tauri GUI)
//!
//! The GUI and the CLI are both thin clients of this crate — neither
//! duplicates application logic.

pub mod api;
pub mod approval;
pub mod capture;
pub mod catalog;
pub mod notify;
pub mod provider_health;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use handover_agents::AgentRegistry;
use handover_config::{
    builtin_session_agents, AgentConfig, AgentKind, Appearance, ApprovalChannel, Config,
    ConfigError,
};
use handover_core::action::{builtin_actions, find_action, Action};
use handover_core::agent::{AgentError, AgentMetaStatus, AgentRequest, SendReceipt, SessionTarget};
use handover_core::approval::{tail_contains_marker, tail_marker_line};
use handover_core::capture::Capture;
use handover_core::exclusions::is_excluded_path;
use handover_core::prompt::{render_action_prompt, render_capture_context};
use handover_core::session::{
    live_sessions_from_cli_rows, parse_cli_list_output, scan_live_sessions_home, LiveSession,
    SessionDiscovery, SessionSpec, DEFAULT_STALENESS,
};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT: u16 = 47444;

/// The maximum size of a file that is read into a capture (1 MiB).
pub const MAX_FILE_CAPTURE_BYTES: u64 = 1024 * 1024;

/// Maximum UTF-8 byte length of text embedded from clipboard / CLI / stdin.
/// Larger text is truncated and a `text_note` is added (mirrors the file cap).
pub const MAX_TEXT_CAPTURE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Agent(#[from] AgentError),
}

/// The running state of Handover. Both the CLI (via HTTP) and the GUI
/// (in-process) operate on this.
pub struct Daemon {
    pub config: Config,
    /// Where preferences and other config mutations are persisted.
    /// Production uses the platform config path; tests inject a temp path so
    /// they never overwrite the user's real config.
    pub config_path: PathBuf,
    /// Bearer token required by the local HTTP API for mutating routes.
    pub api_token: String,
    pub registry: AgentRegistry,
    pub port: u16,
    /// Session-only record of recent handoffs (palette, tray, API).
    pub history: HandoffHistory,
    /// Persisted per-agent session state (pinned sessions) — mode 0600 file
    /// next to config, NOT part of HandoffHistory. Survives restarts.
    pub session_state: SessionState,
    /// Path of the session-state file (production: next to config.toml;
    /// tests: inside the temp config dir).
    pub session_state_path: PathBuf,
    /// Where session discovery resolves `~` globs from. `None` in
    /// production (real home); tests inject a temp dir so discovery never
    /// reads the user's real agent sessions.
    pub session_scan_home: Option<PathBuf>,
    /// When true, session discovery also overlays the built-in agent catalog
    /// (claude/codex/droid/omp/hermes) so zero-config session awareness works.
    /// Tests disable it to stay hermetic.
    pub session_catalog_overlay: bool,
    /// Per-agent model-provider endpoint sniffing + health probes. Lets a
    /// doomed send fail in under a second with a real reason instead of
    /// hanging for the full agent timeout. Never blocks unknown agents.
    pub provider_health: Arc<provider_health::ProviderHealth>,
}

/// Persisted per-agent session state. Currently: the user's pinned session
/// per agent. Written as JSON (mode 0600) next to config — never transcript
/// contents, only session ids.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionState {
    /// agent_id -> pinned session id (set via `handover attach` / API).
    pub pinned: HashMap<String, String>,
}

/// Shared, mutable daemon state managed by the GUI / HTTP server.
#[derive(Clone)]
pub struct SharedDaemon(pub Arc<Mutex<Daemon>>);

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

/// Cheap inputs for [`compute_agents_status`], gathered under the daemon
/// lock so the probes (process check, provider health) can run without it.
pub struct AgentsStatusSnapshot {
    /// Registry statuses (filesystem detection only — no subprocesses).
    pub statuses: Vec<AgentMetaStatus>,
    /// agent id → launch command, for the live-process check.
    pub commands: HashMap<String, String>,
    /// Shared provider-health probe state (internally synchronized).
    pub provider_health: Arc<provider_health::ProviderHealth>,
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
    pub provider_health: Arc<provider_health::ProviderHealth>,
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

impl Daemon {
    pub fn load() -> Result<Self, DaemonError> {
        let config_path = Config::config_path();
        // `ensure` creates config + API token when possible. Prefer loading the
        // existing token; only create if ensure failed to materialize one.
        let config = Config::ensure();
        let api_token = match Config::load_api_token() {
            Some(t) => t,
            None => Config::ensure_api_token().map_err(|e| {
                DaemonError::Message(format!("could not load/create API token: {e}"))
            })?,
        };
        let registry = AgentRegistry::from_configs(&config.agents);
        Ok(Self {
            port: port_from_env(),
            config,
            config_path: config_path.clone(),
            api_token,
            registry,
            history: HandoffHistory::default(),
            session_state: load_session_state(&session_state_path_for(&config_path)),
            session_state_path: session_state_path_for(&config_path),
            session_scan_home: None,
            session_catalog_overlay: true,
            provider_health: Arc::new(provider_health::ProviderHealth::new()),
        })
    }

    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "daemon": true,
            "port": self.port,
            "platform": std::env::consts::OS,
            "actions": builtin_actions().len(),
            "agents": self.registry.statuses().len(),
            "session_aware_agents": self
                .session_specs()
                .iter()
                .map(|s| s.agent_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            "config_path": self.config_path.display().to_string(),
            "default_agent": self.registry.default_id(),
            "auth_required": true,
            // So clients (CLI) can wait at least as long as the slowest agent.
            "max_agent_timeout_secs": self.max_agent_timeout_secs(),
            "default_agent_timeout_secs": 120u64,
        })
    }

    /// Longest configured agent `timeout_secs`, or the generic-command default (120).
    pub fn max_agent_timeout_secs(&self) -> u64 {
        self.config
            .agents
            .iter()
            .filter(|a| a.enabled)
            .filter_map(|a| a.timeout_secs)
            .max()
            .unwrap_or(120)
    }

    pub fn actions(&self) -> Vec<Action> {
        builtin_actions()
    }

    pub fn agents_status(&self) -> Vec<AgentMetaStatus> {
        compute_agents_status(self.agents_status_snapshot())
    }

    /// Gathers the cheap inputs for [`compute_agents_status`] under the
    /// caller's lock: registry statuses (filesystem detection only) and the
    /// agent id → command map for the live-process check.
    pub fn agents_status_snapshot(&self) -> AgentsStatusSnapshot {
        AgentsStatusSnapshot {
            statuses: self.registry.statuses(),
            commands: self
                .config
                .agents
                .iter()
                .map(|a| (a.id.clone(), a.command.clone()))
                .collect(),
            provider_health: Arc::clone(&self.provider_health),
        }
    }

    pub fn preference_for(&self, action_id: &str) -> Option<String> {
        self.config.preference_for(action_id).cloned()
    }

    pub fn set_preference(&mut self, action_id: &str, agent_id: &str) {
        self.config.set_preference(action_id, agent_id);
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save preferences to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists the `general.quick_send` flag (skip the agent picker when a
    /// preferred agent is known for an action). Used by the Settings window.
    pub fn set_quick_send(&mut self, enabled: bool) {
        self.config.general.quick_send = enabled;
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists the `general.appearance` preference (follow the system, or
    /// force light/dark). Used by the Settings window; the desktop app applies
    /// it to the windows and the frontend mirrors it into CSS tokens.
    pub fn set_appearance(&mut self, appearance: Appearance) {
        self.config.general.appearance = appearance;
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists `general.ui_opacity` (palette + Settings glass opacity, 0.4–1.0).
    pub fn set_ui_opacity(&mut self, opacity: f64) {
        self.config.general.ui_opacity = handover_config::clamp_ui_opacity(opacity);
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists the `general.launch_at_startup` flag. Informational for the
    /// CLI/API — the desktop app performs the actual enable/disable through
    /// the autostart plugin and mirrors the result here.
    pub fn set_launch_at_startup(&mut self, enabled: bool) {
        self.config.general.launch_at_startup = enabled;
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists the `general.notifications` flag (desktop handoff notifications).
    pub fn set_notifications(&mut self, enabled: bool) {
        self.config.general.notifications = enabled;
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists the `general.shortcut` accelerator string (the desktop app
    /// re-registers the global hotkey from it). Informational for the CLI/API.
    pub fn set_shortcut(&mut self, shortcut: &str) {
        self.config.general.shortcut = shortcut.to_string();
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Persists the `privacy.excluded_paths` glob list (Settings → Privacy).
    pub fn set_excluded_paths(&mut self, patterns: Vec<String>) {
        self.config.privacy.excluded_paths = patterns;
        if let Err(e) = self.config.save_to(&self.config_path) {
            log::warn!(
                "could not save config to {}: {e}",
                self.config_path.display()
            );
        }
    }

    /// Clears the session handoff history (Settings / palette "Clear history").
    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    /// Adds a new agent to the config, persists it, and rebuilds the registry
    /// so the new agent is immediately available to the palette and the API
    /// without a restart.
    pub fn add_agent(&mut self, agent: AgentConfig) -> Result<(), DaemonError> {
        if agent.id.trim().is_empty() {
            return Err(DaemonError::Message("Agent id must not be empty.".into()));
        }
        if agent.name.trim().is_empty() {
            return Err(DaemonError::Message("Agent name must not be empty.".into()));
        }
        if agent.command.trim().is_empty() {
            return Err(DaemonError::Message(
                "Agent command must not be empty.".into(),
            ));
        }
        if self.config.agent(&agent.id).is_some() {
            return Err(DaemonError::Message(format!(
                "An agent with id `{}` already exists.",
                agent.id
            )));
        }
        self.config.agents.push(agent);
        self.config.save_to(&self.config_path)?;
        self.registry = AgentRegistry::from_configs(&self.config.agents);
        Ok(())
    }

    /// Removes an agent by id, persists the config, forgets any preferences
    /// pointing at it, and rebuilds the registry.
    pub fn remove_agent(&mut self, id: &str) -> Result<(), DaemonError> {
        let before = self.config.agents.len();
        self.config.agents.retain(|a| a.id != id);
        if self.config.agents.len() == before {
            return Err(DaemonError::Message(format!(
                "No agent with id `{id}` is configured."
            )));
        }
        self.config
            .preferences
            .retain(|_, agent_id| agent_id.as_str() != id);
        self.config.save_to(&self.config_path)?;
        self.registry = AgentRegistry::from_configs(&self.config.agents);
        Ok(())
    }

    /// All configured agents (including disabled ones) for the management UI.
    /// The registry only holds enabled agents, so this reads the config directly.
    pub fn configured_agents(&self) -> Vec<AgentConfig> {
        self.config.agents.clone()
    }

    /// Makes `id` the default agent (first enabled entry in config order) and
    /// persists the reorder. Lets users switch the default from Settings.
    pub fn set_default_agent(&mut self, id: &str) -> Result<(), DaemonError> {
        let idx = self
            .config
            .agents
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| {
                DaemonError::Message(format!("No agent with id `{id}` is configured."))
            })?;
        if !self.config.agents[idx].enabled {
            return Err(DaemonError::Message(format!(
                "Agent `{id}` is disabled — enable it before making it the default."
            )));
        }
        if idx == 0 {
            return Ok(()); // already the default
        }
        let agent = self.config.agents.remove(idx);
        self.config.agents.insert(0, agent);
        self.config.save_to(&self.config_path)?;
        self.registry = AgentRegistry::from_configs(&self.config.agents);
        Ok(())
    }

    /// Enables or disables a configured agent. Disabled agents stay fully
    /// configured (command, env, preferences) but are hidden from the palette
    /// dropdown — the registry only exposes enabled agents, so this is a
    /// visibility switch, not a delete.
    pub fn set_agent_enabled(&mut self, id: &str, enabled: bool) -> Result<(), DaemonError> {
        let agent = self
            .config
            .agents
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| {
                DaemonError::Message(format!("No agent with id `{id}` is configured."))
            })?;
        if agent.enabled == enabled {
            return Ok(()); // no-op: already in the requested state
        }
        agent.enabled = enabled;
        self.config.save_to(&self.config_path)?;
        self.registry = AgentRegistry::from_configs(&self.config.agents);
        Ok(())
    }

    /// The session-discovery specs to scan: configured session-aware agents
    /// first (user overrides the catalog), then built-in catalog entries for
    /// agents that are NOT configured (zero-config session awareness).
    fn session_specs(&self) -> Vec<SessionSpec> {
        let mut specs: Vec<SessionSpec> = Vec::new();
        for agent in &self.config.agents {
            if agent.kind != AgentKind::Session {
                continue;
            }
            if let Some(glob) = &agent.session_glob {
                specs.push(SessionSpec::glob(&agent.id, glob));
            } else if let Some(cmd) = &agent.session_cli_list {
                specs.push(SessionSpec::cli_list(&agent.id, cmd.clone()));
            }
        }
        if self.session_catalog_overlay {
            let configured: std::collections::HashSet<&str> =
                self.config.agents.iter().map(|a| a.id.as_str()).collect();
            for catalog in builtin_session_agents() {
                if configured.contains(catalog.id.as_str()) {
                    continue;
                }
                if let Some(glob) = &catalog.session_glob {
                    specs.push(SessionSpec::glob(&catalog.id, glob));
                } else if let Some(cmd) = &catalog.session_cli_list {
                    specs.push(SessionSpec::cli_list(&catalog.id, cmd.clone()));
                }
            }
        }
        specs
    }

    /// Scans for live sessions on demand (never a background poller).
    /// Returns freshest-first. Two-phase: [`Self::live_sessions_snapshot`]
    /// gathers cheap inputs under the lock; [`compute_live_sessions`] does
    /// the glob scan, cli-list subprocesses and approval tail-peeks
    /// without it.
    pub fn live_sessions(&self) -> Vec<LiveSession> {
        compute_live_sessions(&self.live_sessions_snapshot())
    }

    /// Gathers the cheap inputs for [`compute_live_sessions`]: discovery
    /// specs, the home dir to resolve globs against, and the per-agent
    /// permission markers (the opt-in approval peek). Pure config reads —
    /// no subprocesses, no filesystem walks.
    pub fn live_sessions_snapshot(&self) -> LiveSessionsSnapshot {
        LiveSessionsSnapshot {
            specs: self.session_specs(),
            home: self
                .session_scan_home
                .clone()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))),
            markers: self
                .config
                .agents
                .iter()
                .filter(|a| a.kind == AgentKind::Session)
                .filter_map(|a| {
                    a.permission_marker
                        .clone()
                        .map(|marker| (a.id.clone(), marker))
                })
                .collect(),
        }
    }

    /// Resolves an approval request: validates the agent's config, finds
    /// the live session, and re-checks it is STILL blocked. Returns an
    /// [`ApprovalRequest`] ready for [`execute_approval`] — which runs
    /// outside the daemon lock so injection + verification polling never
    /// block the palette, the API, or other handoffs.
    pub fn resolve_approval(
        &self,
        agent_id: &str,
        session_id: &str,
        approve: bool,
    ) -> Result<ApprovalRequest, DaemonError> {
        complete_approval(self.resolve_approval_plan(agent_id, session_id, approve)?)
    }

    /// Fast (lock-held) half of an approval: the opt-in config validation
    /// (marker + channel + target) and the session snapshot. The slow
    /// half — liveness re-check, still-blocked re-check, command build —
    /// is [`complete_approval`], off the lock.
    pub fn resolve_approval_plan(
        &self,
        agent_id: &str,
        session_id: &str,
        approve: bool,
    ) -> Result<ApprovalPlan, DaemonError> {
        let msg = |m: String| DaemonError::Message(m);
        // 1. The agent must have opted in (marker + channel + target).
        let agent = self
            .config
            .agents
            .iter()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| msg(format!("Agent `{agent_id}` is not configured")))?;
        let marker = agent.permission_marker.as_deref().ok_or_else(|| {
            msg(format!(
                "Agent `{agent_id}` has no permission_marker — approval is opt-in per agent."
            ))
        })?;
        let channel = agent.approval_channel.ok_or_else(|| {
            msg(format!(
                "Agent `{agent_id}` has no approval_channel — set `tty` or `tmux` in config."
            ))
        })?;
        let target = agent.approval_target.clone().ok_or_else(|| {
            msg(format!(
                "Agent `{agent_id}` has no approval_target — run `handover attach` inside the \
                 session, or set approval_target in config."
            ))
        })?;

        Ok(ApprovalPlan {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            approve,
            marker: marker.to_string(),
            channel,
            target,
            snapshot: self.live_sessions_snapshot(),
        })
    }

    /// One-shot: resolve + execute under a single call (used by the HTTP API
    /// and tests that don't need the two-phase split).
    pub fn approve_session(
        &self,
        agent_id: &str,
        session_id: &str,
        approve: bool,
    ) -> Result<approval::ApprovalResult, DaemonError> {
        self.approve_session_budget(
            agent_id,
            session_id,
            approve,
            approval::APPROVAL_VERIFY_BUDGET,
        )
    }

    /// [`approve_session`] with an explicit verify budget (tests use a short
    /// one).
    pub fn approve_session_budget(
        &self,
        agent_id: &str,
        session_id: &str,
        approve: bool,
        budget: std::time::Duration,
    ) -> Result<approval::ApprovalResult, DaemonError> {
        let req = self.resolve_approval(agent_id, session_id, approve)?;
        execute_approval(&req, budget)
    }

    /// Persisted pinned session for an agent, if any.
    pub fn pinned_session(&self, agent_id: &str) -> Option<String> {
        self.session_state.pinned.get(agent_id).cloned()
    }

    /// Pins a session for an agent and persists the state file (mode 0600).
    /// Lets `handover attach` and the UI fix an ambiguous target.
    pub fn pin_session(&mut self, agent_id: &str, session_id: &str) -> Result<(), DaemonError> {
        self.session_state
            .pinned
            .insert(agent_id.to_string(), session_id.to_string());
        save_session_state(&self.session_state_path, &self.session_state)
    }

    /// Clears a pinned session for an agent and persists the state file.
    pub fn unpin_session(&mut self, agent_id: &str) -> Result<(), DaemonError> {
        self.session_state.pinned.remove(agent_id);
        save_session_state(&self.session_state_path, &self.session_state)
    }

    /// Records the approval channel's target (a tty device / tmux target) for
    /// an agent, persisting to config. `handover attach` (run inside the
    /// session) calls this so approvals can inject into the right place.
    pub fn set_approval_target(&mut self, agent_id: &str, target: &str) -> Result<(), DaemonError> {
        let agent = self
            .config
            .agents
            .iter_mut()
            .find(|a| a.id == agent_id)
            .ok_or_else(|| {
                DaemonError::Message(format!("No agent with id `{agent_id}` is configured."))
            })?;
        agent.approval_target = Some(target.trim().to_string());
        self.config
            .save_to(&self.config_path)
            .map_err(|e| DaemonError::Message(format!("{e}")))
    }

    /// Resolves which agent should receive a handoff: explicit choice, then
    /// the remembered per-action preference, then the default agent.
    pub fn resolve_agent_id(&self, action_id: &str, explicit: Option<&str>) -> Option<String> {
        if let Some(id) = explicit {
            if self.registry.get(id).is_some() {
                return Some(id.to_string());
            }
        }
        if let Some(pref) = self.preference_for(action_id) {
            if self.registry.get(&pref).is_some() {
                return Some(pref);
            }
        }
        self.registry.default_id()
    }

    /// Reads a captured file into the capture (respecting exclusions and a
    /// size cap) so it can be attached to the prompt.
    fn enrich_file_capture(&self, capture: &mut Capture) -> Result<(), String> {
        let path = match &capture.content.path {
            Some(p) => p.clone(),
            None => return Ok(()),
        };
        let path_ref = Path::new(&path);
        let patterns = &self.config.privacy.excluded_paths;

        if is_excluded_path(path_ref, patterns) {
            return Err(format!(
                "Refusing to attach `{path}` — it matches a privacy exclusion (see config privacy.excluded_paths)."
            ));
        }

        if let Ok(resolved) = path_ref.canonicalize() {
            if is_excluded_path(&resolved, patterns) {
                return Err(format!(
                    "Refusing to attach `{}` — resolved path matches a privacy exclusion (see config privacy.excluded_paths).",
                    resolved.display()
                ));
            }
        }

        let (meta, bytes) = read_file_nofollow(path_ref, MAX_FILE_CAPTURE_BYTES)?;
        if !meta.is_file() {
            return Ok(());
        }
        if meta.len() > MAX_FILE_CAPTURE_BYTES {
            capture.metadata.insert(
                "file_note".into(),
                format!(
                    "file is {:.1} MiB and was not attached (max 1 MiB)",
                    meta.len() as f64 / 1_048_576.0
                ),
            );
            return Ok(());
        }
        let Some(bytes) = bytes else {
            return Ok(());
        };
        if bytes.contains(&0) {
            capture.metadata.insert(
                "file_note".into(),
                "binary file — contents not embedded; path attached only".into(),
            );
            return Ok(());
        }
        match String::from_utf8(bytes) {
            Ok(text) => capture.content.text = Some(text),
            Err(_) => {
                capture.metadata.insert(
                    "file_note".into(),
                    "non-UTF-8 file — path attached only".into(),
                );
            }
        }
        Ok(())
    }

    /// Fast half of a handoff, done *under the daemon lock*: normalize the
    /// capture, enrich file captures, resolve the action and agent. The
    /// slow half — provider probe, prompt render, session resolution — is
    /// [`complete_send`], off the lock.
    pub fn resolve_send(
        &self,
        action_id: &str,
        agent_id: Option<&str>,
        session_id: Option<&str>,
        capture: Capture,
    ) -> Result<(Arc<dyn handover_core::agent::Agent>, AgentRequest), DaemonError> {
        complete_send(self.resolve_send_plan(action_id, agent_id, session_id, capture)?)
    }

    /// [`resolve_send`]'s lock-held half: cheap capture prep and config
    /// resolution, plus the snapshots the slow half needs.
    pub fn resolve_send_plan(
        &self,
        action_id: &str,
        agent_id: Option<&str>,
        session_id: Option<&str>,
        mut capture: Capture,
    ) -> Result<SendPlan, DaemonError> {
        capture.normalize();
        cap_text_capture(&mut capture);
        self.enrich_file_capture(&mut capture)
            .map_err(DaemonError::Message)?;

        let action = find_action(&builtin_actions(), action_id)
            .ok_or_else(|| DaemonError::Message(format!("Unknown action `{action_id}`")))?
            .clone();

        let resolved = self.resolve_agent_id(action_id, agent_id).ok_or_else(|| {
            DaemonError::Message(format!(
                "No agent configured. Add an agent to {} and restart Handover.",
                self.config_path.display()
            ))
        })?;

        let agent = self
            .registry
            .get(&resolved)
            .ok_or_else(|| DaemonError::Message(format!("Agent `{resolved}` is not configured")))?;

        let pinned = self.pinned_session(&resolved);
        Ok(SendPlan {
            agent_id: resolved,
            agent,
            action,
            capture,
            explicit_session: session_id.map(str::to_string),
            pinned_session: pinned,
            snapshot: self.live_sessions_snapshot(),
            provider_health: Arc::clone(&self.provider_health),
        })
    }

    /// Performs a handoff: render the prompt from the action + capture,
    /// resolve the agent, and execute the send (no live streaming — used by
    /// the CLI/HTTP path, which is request/response).
    pub fn send(
        &self,
        action_id: &str,
        agent_id: Option<&str>,
        session_id: Option<&str>,
        capture: Capture,
    ) -> Result<SendOutcome, DaemonError> {
        let (agent, request) = self.resolve_send(action_id, agent_id, session_id, capture)?;
        let outcome = execute_handoff(agent, request, None, next_handoff_id());
        self.history.record(outcome.clone());
        Ok(outcome)
    }

    /// Renders the prompt for an action + capture without sending. Applies
    /// the same normalization and file enrichment as a real handoff so what
    /// you preview is exactly what would be sent.
    pub fn render_prompt(
        &self,
        action_id: &str,
        mut capture: Capture,
    ) -> Result<String, DaemonError> {
        capture.normalize();
        cap_text_capture(&mut capture);
        self.enrich_file_capture(&mut capture)
            .map_err(DaemonError::Message)?;
        let action = find_action(&builtin_actions(), action_id)
            .ok_or_else(|| DaemonError::Message(format!("Unknown action `{action_id}`")))?
            .clone();
        Ok(render_action_prompt(&action, &capture))
    }
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
    let cmd = approval::build_approval_command(&plan.channel, &plan.target, plan.approve)
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

/// Executes a resolved approval: injects the keystroke and polls the
/// transcript for signs of life. A free function (not a method on `Daemon`)
/// so callers can run it *without* the daemon lock — injection + verify
/// polling must never block the palette, the API, or other handoffs.
pub fn execute_approval(
    req: &ApprovalRequest,
    budget: std::time::Duration,
) -> Result<approval::ApprovalResult, DaemonError> {
    let verb = if req.approve { "Approved" } else { "Denied" };
    let status = std::process::Command::new(&req.cmd[0])
        .args(&req.cmd[1..])
        .status();
    match status {
        Ok(s) if s.success() => {
            let resumed = req.baseline.is_some()
                && approval::verify_session_resumed(
                    &req.path,
                    &req.marker,
                    req.baseline.unwrap(),
                    budget,
                );
            if resumed {
                Ok(approval::ApprovalResult {
                    verified: true,
                    message: format!("{verb} — the agent is working again."),
                })
            } else {
                Ok(approval::ApprovalResult {
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

/// Truncate oversized text bodies (clipboard / CLI / stdin) so prompts cannot
/// balloon without bound. Sets `metadata["text_note"]` when truncation occurs.
fn cap_text_capture(capture: &mut Capture) {
    let Some(text) = capture.content.text.as_ref() else {
        return;
    };
    let original_bytes = text.len();
    if original_bytes <= MAX_TEXT_CAPTURE_BYTES {
        return;
    }
    // Truncate on a UTF-8 character boundary at or below the byte budget.
    let mut end = MAX_TEXT_CAPTURE_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = text[..end].to_string();
    capture.content.text = Some(truncated);
    capture.metadata.insert(
        "text_note".into(),
        format!(
            "text was truncated to {:.1} MiB (original was {:.1} MiB)",
            MAX_TEXT_CAPTURE_BYTES as f64 / 1_048_576.0,
            original_bytes as f64 / 1_048_576.0
        ),
    );
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

pub fn port_from_env() -> u16 {
    std::env::var("HANDOVER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
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
fn humanize_receipt_failure(
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

/// Open a path for reading **without following a final-component symlink**,
/// then read up to `max_bytes` from the same handle. Returns metadata from
/// that open file (so a leaf symlink swap after check cannot redirect the read).
///
/// When the file is larger than `max_bytes`, returns `(meta, None)` so the
/// caller can attach a size note without loading the body.
///
/// **Scope of the guarantee:** `O_NOFOLLOW` applies to the last path component
/// only. Intermediate directories are resolved by the kernel during `open(2)`
/// and are not individually pinned with `openat`. See `enrich_file_capture`
/// for the product threat model (trusted parent directories).
///
/// Public so the desktop shell can route palette drops through the exact same
/// reader as daemon file capture — one privacy discipline, both entrances.
pub fn read_file_nofollow(
    path: &Path,
    max_bytes: u64,
) -> Result<(std::fs::Metadata, Option<Vec<u8>>), String> {
    use std::io::Read;

    #[cfg(unix)]
    let mut file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| {
                let path_disp = path.display();
                if e.raw_os_error() == Some(libc::ELOOP)
                    || e.kind() == std::io::ErrorKind::InvalidInput
                {
                    // macOS/Linux: O_NOFOLLOW on a symlink → ELOOP (or platform variant).
                    format!(
                        "Refusing to attach `{path_disp}` — symbolic links are not allowed for file capture (privacy)."
                    )
                } else {
                    format!("Could not read `{path_disp}`: {e}")
                }
            })?
    };

    #[cfg(not(unix))]
    let mut file = {
        // Best-effort: refuse if lstat says symlink, then open. Windows has no
        // portable O_NOFOLLOW equivalent in std; TOCTOU remains a residual risk.
        let meta = std::fs::symlink_metadata(path)
            .map_err(|e| format!("Could not read `{}`: {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!(
                "Refusing to attach `{}` — symbolic links are not allowed for file capture (privacy).",
                path.display()
            ));
        }
        std::fs::File::open(path)
            .map_err(|e| format!("Could not read `{}`: {e}", path.display()))?
    };

    let meta = file
        .metadata()
        .map_err(|e| format!("Could not stat `{}`: {e}", path.display()))?;
    if !meta.is_file() {
        return Ok((meta, None));
    }
    if meta.len() > max_bytes {
        return Ok((meta, None));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("Could not read `{}`: {e}", path.display()))?;
    Ok((meta, Some(bytes)))
}

/// Placeholder for unauthenticated HTTP agent listings so configured command
/// lines (including `ENV=secret cmd` forms) are never disclosed on loopback GETs.
pub fn redact_command_summary(_summary: &str) -> String {
    "configured command".to_string()
}

/// State wrapper used by both the HTTP server and the GUI.
pub fn shared(daemon: Daemon) -> SharedDaemon {
    SharedDaemon(Arc::new(Mutex::new(daemon)))
}

/// The session-state file path for a given config path: the sibling
/// `session_state.json` next to `config.toml`. Mirrors the production
/// `Config::session_state_path()` while staying next to whatever config path
/// is in use (so tests inject a temp one).
fn session_state_path_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("session_state.json")
}

/// Best-effort live-process check: is a process for this agent's command
/// running right now? The program is the first real token of the command
/// (`sh -c` wrappers are skipped, so `sh -c 'hermes …'` still finds hermes).
/// Matching is by exact process name first (`pgrep -x`, agents run directly
/// in a terminal), then by the program's basename appearing in the command
/// line (`pgrep -f` — catches python entry-point launchers like Hermes'
/// venv). Fail closed: any error or a wrapper-only command reports false.
fn agent_process_running(command: &str) -> bool {
    let Some(program) = handover_agents::generic_command::first_program(command) else {
        return false;
    };
    let basename = Path::new(&program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.clone());
    if basename.is_empty()
        || matches!(
            basename.as_str(),
            "sh" | "bash" | "zsh" | "python" | "python3"
        )
    {
        // A shell/interpreter alone is not the agent itself.
        return false;
    }
    let probe = |extra: &[&str]| {
        std::process::Command::new("pgrep")
            .args(extra)
            .arg(&basename)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };
    // Exact process-name match first (agents run directly in a terminal);
    // then a whole-component scan for interpreter launchers (Hermes runs as
    // `…/venv/bin/python …/hermes`, where the process NAME is python). A bare
    // substring search (`pgrep -f`) must NEVER be used: it matched unrelated
    // processes (`omp` inside MTLCompilerService, `codex` inside a vite/npm
    // invocation) and lit agents green that were not running at all.
    probe(&["-x"]) || any_command_line_contains(&basename)
}

/// True when `line` contains `token` as a whole path component / whole
/// argument — never buried inside a longer word. Splitting on whitespace
/// and `/`: `MTLCompilerService` is one component (≠ `omp`), while the
/// trailing `hermes` in `…/hermes-agent/hermes` stands alone (= `hermes`).
fn command_line_has_component(line: &str, token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    line.split(|c: char| c.is_whitespace() || c == '/')
        .any(|part| part == token)
}

/// Scans every live process's command line for the program appearing as a
/// whole component. One `ps` call, fail closed on error.
fn any_command_line_contains(token: &str) -> bool {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-axo", "command="])
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .any(|line| command_line_has_component(line, token))
}

/// Hard cap for a session cli-list command (e.g. `hermes sessions list`).
/// Discovery is a status read — a hung listing fails soft, never blocks.
const CLI_LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Resolves a cli-list program to an absolute path. Finder-launched apps
/// inherit a restricted PATH (typically just `/usr/bin:/bin`), so a bare
/// name like `hermes` — installed in `~/.local/bin` — would silently fail to
/// spawn and discovery would return nothing. Absolute paths pass through;
/// bare names are looked up in the inherited PATH plus the common macOS
/// install locations (user bin dirs and Homebrew). Falls back to the bare
/// name so `Command::new` reports the real spawn error.
fn resolve_cli_program(program: &str) -> String {
    if program.contains('/') {
        return program.to_string();
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join("bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/sbin"));
    for dir in dirs {
        let candidate = dir.join(program);
        if !candidate.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = candidate.metadata() {
                if meta.permissions().mode() & 0o111 == 0 {
                    continue; // present but not executable — keep looking
                }
            }
        }
        return candidate.to_string_lossy().into_owned();
    }
    program.to_string()
}

/// Spawns a command with piped stdout/stderr and a hard wall-clock budget.
/// Returns `None` on spawn failure, timeout (the child is killed), or wait
/// error; `Some(Output)` otherwise, mirroring `Command::output`.
///
/// Pipes are drained concurrently via background threads so the child never
/// blocks on a full pipe buffer (the bug that `GenericCommandAgent` already
/// solves with `drain_capped`).
pub(crate) fn run_command_bounded(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::io::Read;
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // Drain stdout and stderr in background threads so the child never
    // blocks on a full OS pipe buffer (~64KB). Without this, a command
    // that writes more than the buffer would deadlock: the child waits
    // on write() and the parent waits on try_wait().
    let stdout_h = child.stdout.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = r;
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_h = child.stderr.take().map(|r| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut reader = r;
            let _ = reader.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_h.and_then(|h| h.join().ok()).unwrap_or_default();
                let stderr = stderr_h.and_then(|h| h.join().ok()).unwrap_or_default();
                return Some(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Runs a session cli-list command with a hard [`CLI_LIST_TIMEOUT`] budget.
/// Returns `None` on spawn failure or timeout (the child is killed);
/// `Some(Output)` otherwise — including non-zero exits, which the caller
/// treats fail-soft.
fn run_cli_list(program: &str, args: &[String]) -> Option<std::process::Output> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    run_command_bounded(&mut command, CLI_LIST_TIMEOUT)
}

/// Reads the persisted session state; missing/corrupt file → empty state.
fn load_session_state(path: &Path) -> SessionState {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_else(|e| {
            log::warn!("could not parse session state {}: {e}", path.display());
            SessionState::default()
        }),
        Err(_) => SessionState::default(),
    }
}

/// Writes the session-state file (mode 0600). Best-effort: a failure to
/// persist a pin is logged, never fatal.
fn save_session_state(path: &Path, state: &SessionState) -> Result<(), DaemonError> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| DaemonError::Message(format!("session state serialize: {e}")))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| DaemonError::Message(format!("{e}")))?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| DaemonError::Message(format!("{e}")))?;
        file.write_all(raw.as_bytes())
            .map_err(|e| DaemonError::Message(format!("{e}")))?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, raw).map_err(|e| DaemonError::Message(format!("{e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handover_config::AgentConfig;
    use handover_core::capture::{Capture, SourceKind};

    /// Builds a daemon whose config persistence path is under a unique temp
    /// directory — never the user's real platform config path.
    fn test_daemon(config: Config) -> Daemon {
        let dir = std::env::temp_dir().join(format!("handover-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp config dir");
        // Prompt temp files must not land in the real user cache. The env
        // var is process-global while tests run on parallel threads, so set
        // it exactly ONCE per test binary — per-daemon values flap between
        // concurrent tests and make one daemon's prompt file land under
        // another daemon's root (observed as a one-off send flake).
        static PROMPT_CACHE: std::sync::LazyLock<()> = std::sync::LazyLock::new(|| {
            std::env::set_var(
                "HANDOVER_PROMPT_CACHE_DIR",
                std::env::temp_dir()
                    .join(format!("handover-test-prompts-{}", uuid::Uuid::new_v4())),
            );
        });
        std::sync::LazyLock::force(&PROMPT_CACHE);
        let config_path = dir.join("config.toml");
        config.save_to(&config_path).expect("seed test config");
        let api_token = Config::ensure_api_token_at(&dir.join("api_token")).expect("test token");
        let registry = AgentRegistry::from_configs(&config.agents);
        Daemon {
            provider_health: Arc::new(provider_health::ProviderHealth::with_home(dir.clone())),
            config,
            config_path: config_path.clone(),
            api_token,
            registry,
            port: 0,
            history: HandoffHistory::default(),
            session_state: SessionState::default(),
            session_state_path: session_state_path_for(&config_path),
            // Discovery never reads the real user's agent sessions in tests.
            session_scan_home: Some(dir),
            session_catalog_overlay: false,
        }
    }

    fn real_config_path() -> PathBuf {
        Config::config_path()
    }

    #[test]
    fn send_handoff_via_demo_agent() {
        let daemon = test_daemon(Config::default_config());
        let capture = Capture::terminal("ECONNREFUSED 127.0.0.1:5432", Some("Terminal".into()));
        let outcome = daemon
            .send("fix", None, None, capture)
            .expect("send should succeed");
        assert!(outcome.ok, "{}", outcome.error.unwrap_or_default());
        assert_eq!(outcome.agent_id, "demo-echo");
        assert_eq!(outcome.agent_name, "Echo (demo)");
        assert!(outcome.prompt.contains("ECONNREFUSED 127.0.0.1:5432"));
        assert!(outcome
            .prompt
            .contains("Investigate this issue and fix it."));
        assert!(outcome.receipt.is_some());
    }

    #[test]
    fn chat_send_is_verbatim_not_preamble_wrapped() {
        // A chat message (metadata `chat` marker, set only by the palette
        // composer) must reach the agent AS-IS — the "Analyze the provided
        // context…" preamble made hermes answer "I don't see any new context"
        // instead of answering conversationally.
        let daemon = test_daemon(Config::default_config());
        let mut capture = Capture::text(SourceKind::Manual, "how are you doing?", None);
        capture.metadata.insert("chat".into(), "true".into());
        let outcome = daemon
            .send("ask", None, None, capture)
            .expect("send should succeed");
        assert!(outcome.ok, "{}", outcome.error.unwrap_or_default());
        assert_eq!(outcome.prompt, "how are you doing?");
        // And a NON-chat ask still carries the preamble + context.
        let plain = daemon
            .send(
                "ask",
                None,
                None,
                Capture::text(SourceKind::Manual, "x", None),
            )
            .expect("send should succeed");
        assert!(plain.prompt.contains("Analyze the provided context"));
        assert!(plain.prompt.contains("x"));
    }

    #[test]
    fn unknown_action_is_rejected() {
        let daemon = test_daemon(Config::default_config());
        let capture = Capture::text(SourceKind::Manual, "x", None);
        let err = daemon
            .send("does-not-exist", None, None, capture)
            .unwrap_err();
        assert!(err.to_string().contains("Unknown action"));
    }

    #[test]
    fn excluded_path_is_refused_before_reading() {
        let daemon = test_daemon(Config::default_config());
        // A file that matches the `.env` exclusion, even if it does not exist
        // on disk — the check happens before any read.
        let capture = Capture::file("/tmp/project/.env", None);
        let err = daemon.send("ask", None, None, capture).unwrap_err();
        assert!(
            err.to_string().contains("Refusing"),
            "expected refusal, got: {err}"
        );
    }

    #[test]
    fn symlink_to_excluded_secret_is_refused() {
        let dir = std::env::temp_dir().join(format!("ho-symlink-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let secret = dir.join(".env");
        std::fs::write(&secret, "SECRET=should-not-leak\n").unwrap();
        let link = dir.join("harmless-looking-error.log");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        #[cfg(not(unix))]
        {
            let _ = std::fs::remove_dir_all(&dir);
            return; // symlink policy is a Unix-relevant privacy control
        }

        let daemon = test_daemon(Config::default_config());
        let capture = Capture::file(link.to_string_lossy().to_string(), None);
        let err = daemon.send("ask", None, None, capture).unwrap_err();
        let msg = err.to_string();
        // Either O_NOFOLLOW ("symbolic link") or canonicalize+exclusion on the
        // target (.env) is acceptable — both refuse before secret content is read.
        assert!(
            msg.contains("Refusing")
                && (msg.contains("symbolic link")
                    || msg.contains("privacy exclusion")
                    || msg.contains("resolved")),
            "expected privacy refusal for symlink-to-secret, got: {msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_preference_does_not_write_real_config() {
        let real_path = real_config_path();
        let before = std::fs::read(&real_path).ok();

        let mut config = Config::default_config();
        config.agents.push(AgentConfig {
            id: "second".into(),
            name: "Second".into(),
            kind: handover_config::AgentKind::Command,
            command: "cat".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: None,
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let mut daemon = test_daemon(config);
        assert_ne!(
            daemon.config_path, real_path,
            "test daemon must not use the real config path"
        );

        daemon.set_preference("fix", "second");

        // Preference landed in the temp config file.
        let saved = Config::load_from(&daemon.config_path).expect("load temp config");
        assert_eq!(
            saved.preference_for("fix").map(String::as_str),
            Some("second")
        );

        // Real platform config must be unchanged.
        let after = std::fs::read(&real_path).ok();
        assert_eq!(
            before,
            after,
            "set_preference must not mutate the real user config at {}",
            real_path.display()
        );

        // Cleanup temp dir created by test_daemon.
        if let Some(parent) = daemon.config_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn file_capture_reads_text_into_prompt() {
        let dir = std::env::temp_dir().join(format!("ho-daemon-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("error.log");
        std::fs::write(&path, "panic: something exploded").unwrap();

        let daemon = test_daemon(Config::default_config());
        let capture = Capture::file(path.to_string_lossy().to_string(), None);
        let outcome = daemon
            .send("explain", None, None, capture)
            .expect("send should succeed");
        assert!(outcome.ok, "{}", outcome.error.unwrap_or_default());
        assert!(outcome.prompt.contains("panic: something exploded"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn oversized_file_note_appears_in_prompt() {
        let dir = std::env::temp_dir().join(format!("ho-large-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("huge.log");
        // Just over the 1 MiB attach cap.
        let chunk = vec![b'A'; 1024];
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&path).unwrap();
            for _ in 0..(1024 + 1) {
                f.write_all(&chunk).unwrap();
            }
        }

        let daemon = test_daemon(Config::default_config());
        let capture = Capture::file(path.to_string_lossy().to_string(), None);
        let prompt = daemon
            .render_prompt("ask", capture)
            .expect("render should succeed");
        assert!(
            prompt.contains("File note:"),
            "file_note label missing from prompt: {prompt}"
        );
        assert!(
            prompt.contains("was not attached"),
            "size note missing from prompt: {prompt}"
        );
        assert!(
            !prompt.contains(&"A".repeat(1000)),
            "oversized file body must not be embedded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_default_agent_reorders_and_persists() {
        let mut config = Config::default_config();
        config.agents.push(AgentConfig {
            id: "second".into(),
            name: "Second".into(),
            kind: handover_config::AgentKind::Command,
            command: "cat".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: None,
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let mut daemon = test_daemon(config);
        assert_eq!(
            daemon.resolve_agent_id("fix", None).as_deref(),
            Some("demo-echo")
        );

        daemon.set_default_agent("second").expect("set default");
        assert_eq!(
            daemon.resolve_agent_id("fix", None).as_deref(),
            Some("second")
        );

        let saved = Config::load_from(&daemon.config_path).expect("load temp config");
        assert_eq!(saved.agents[0].id, "second");
        assert_eq!(saved.agents.len(), 2);

        // Unknown id is rejected without mutating anything.
        let err = daemon.set_default_agent("ghost").unwrap_err();
        assert!(err.to_string().contains("ghost"));
        assert_eq!(daemon.config.agents.len(), 2);

        if let Some(parent) = daemon.config_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn set_quick_send_persists() {
        let mut daemon = test_daemon(Config::default_config());
        assert!(
            daemon.config.general.quick_send,
            "default config enables quick_send"
        );
        daemon.set_quick_send(false);
        let saved = Config::load_from(&daemon.config_path).expect("load temp config");
        assert!(!saved.general.quick_send);
        if let Some(parent) = daemon.config_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn set_agent_enabled_hides_without_deleting() {
        // demo-echo is enabled by default in the default test config.
        let mut daemon = test_daemon(Config::default_config());
        assert!(daemon.resolve_agent_id("fix", None).is_some());

        // Disable → gone from the registry, still fully configured.
        daemon
            .set_agent_enabled("demo-echo", false)
            .expect("disable");
        assert_eq!(
            daemon.resolve_agent_id("fix", None),
            None,
            "disabled agent must leave the palette"
        );
        assert_eq!(
            daemon.configured_agents().len(),
            1,
            "config entry must be kept"
        );
        assert!(!daemon.configured_agents()[0].enabled);
        let saved = Config::load_from(&daemon.config_path).expect("load temp config");
        assert!(!saved.agents[0].enabled, "disable must persist");

        // Re-enable → back in the registry.
        daemon.set_agent_enabled("demo-echo", true).expect("enable");
        assert_eq!(
            daemon.resolve_agent_id("fix", None).as_deref(),
            Some("demo-echo")
        );

        // Unknown id is rejected; no-op toggle to the same state succeeds.
        let err = daemon.set_agent_enabled("ghost", true).unwrap_err();
        assert!(err.to_string().contains("ghost"));
        daemon
            .set_agent_enabled("demo-echo", true)
            .expect("no-op ok");

        if let Some(parent) = daemon.config_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn set_appearance_persists() {
        let mut daemon = test_daemon(Config::default_config());
        assert_eq!(daemon.config.general.appearance, Appearance::System);
        daemon.set_appearance(Appearance::Dark);
        let saved = Config::load_from(&daemon.config_path).expect("load temp config");
        assert_eq!(saved.general.appearance, Appearance::Dark);
        assert_eq!(saved.general.appearance.as_str(), "dark");
        if let Some(parent) = daemon.config_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn explicit_agent_beats_preference_beats_default() {
        let mut config = Config::default_config();
        config.agents.push(AgentConfig {
            id: "second".into(),
            name: "Second".into(),
            kind: handover_config::AgentKind::Command,
            command: "cat".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: None,
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let daemon = test_daemon(config);

        // No preference → default (first configured agent).
        assert_eq!(
            daemon.resolve_agent_id("fix", None).as_deref(),
            Some("demo-echo")
        );

        // Preference set for the action → preferred agent.
        let mut daemon = daemon;
        daemon.set_preference("fix", "second");
        assert_eq!(
            daemon.resolve_agent_id("fix", None).as_deref(),
            Some("second")
        );

        // Explicit choice always wins.
        assert_eq!(
            daemon.resolve_agent_id("fix", Some("demo-echo")).as_deref(),
            Some("demo-echo")
        );
    }

    #[test]
    fn history_records_newest_first_and_caps_at_ten() {
        let history = HandoffHistory::default();
        for i in 0..12 {
            history.record(SendOutcome {
                id: format!("handoff-{i}"),
                ok: true,
                agent_id: format!("agent-{i}"),
                agent_name: format!("Agent {i}"),
                action_id: "ask".into(),
                capture: None,
                created_at: chrono::Utc::now(),
                prompt: "p".into(),
                receipt: None,
                error: None,
                session_id: None,
            });
        }
        let recent = history.recent();
        assert_eq!(recent.len(), HANDOFF_HISTORY_CAP);
        assert_eq!(recent[0].agent_id, "agent-11", "newest must be first");
        assert_eq!(recent[9].agent_id, "agent-2", "oldest kept entry");
    }

    #[test]
    fn send_records_handoff_into_history() {
        let daemon = test_daemon(Config::default_config());
        let capture = Capture::text(SourceKind::Manual, "history test", None);
        let outcome = daemon.send("ask", None, None, capture).expect("send");
        assert!(outcome.ok);
        let recent = daemon.history.recent();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].agent_id, "demo-echo");
    }

    #[test]
    fn status_reports_actions_and_agents() {
        let daemon = test_daemon(Config::default_config());
        let status = daemon.status();
        assert_eq!(status["actions"], 5);
        assert_eq!(status["agents"], 1);
        assert_eq!(status["platform"], std::env::consts::OS);
    }

    #[test]
    fn slow_send_does_not_block_health() {
        // A slow agent (2s) must never block /health — handoffs execute with
        // the daemon lock released, one thread per request.
        use std::io::{Read, Write};
        use std::net::TcpStream;

        fn raw_request(
            port: u16,
            method: &str,
            path: &str,
            body: Option<&str>,
            token: Option<&str>,
        ) -> String {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let body_str = body.unwrap_or("");
            let auth = token
                .map(|t| format!("Authorization: Bearer {t}\r\n"))
                .unwrap_or_default();
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_str}",
                body_str.len()
            );
            stream.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            buf
        }

        let mut config = Config::default_config();
        config.agents = vec![AgentConfig {
            id: "slow".into(),
            name: "Slow".into(),
            kind: handover_config::AgentKind::Command,
            command: "sleep 2".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: Some(10),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: None,
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        }];
        let daemon = test_daemon(config);
        let token = daemon.api_token.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = api::serve(shared(daemon), 0, Arc::clone(&shutdown)).unwrap();

        let capture = Capture::text(SourceKind::Manual, "blocking test", None);
        let body = serde_json::json!({
            "action_id": "ask",
            "agent_id": "slow",
            "capture": capture,
        })
        .to_string();
        let port = handle.port;
        let token_for_send = token.clone();
        let send_thread = std::thread::spawn(move || {
            raw_request(port, "POST", "/send", Some(&body), Some(&token_for_send))
        });

        let start = std::time::Instant::now();
        let health = raw_request(handle.port, "GET", "/health", None, None);
        assert!(
            start.elapsed().as_millis() < 1000,
            "/health was blocked by an in-flight /send"
        );
        assert!(health.contains("\"ok\":true"), "health: {health}");

        let send_response = send_thread.join().unwrap();
        assert!(
            send_response.contains("\"ok\":true"),
            "send: {send_response}"
        );

        // Unauthenticated mutating call must be rejected.
        let denied = raw_request(handle.port, "POST", "/quit", Some("{}"), None);
        assert!(
            denied.contains("401") || denied.contains("Unauthorized"),
            "expected unauthorized, got: {denied}"
        );

        // Authenticated quit for clean shutdown.
        let _ = raw_request(handle.port, "POST", "/quit", Some("{}"), Some(&token));
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn slow_sessions_does_not_block_status_or_agents() {
        // H2/H3 acceptance: an agent with a slow cli-list (sessions scan)
        // must not block other requests. Before the snapshot/compute split,
        // GET /sessions held the daemon lock across the full scan — /status
        // and /agents would queue behind it for the whole subprocess wait.
        use std::io::{Read, Write};
        use std::net::TcpStream;

        fn raw_request(
            port: u16,
            method: &str,
            path: &str,
            body: Option<&str>,
            token: Option<&str>,
        ) -> String {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let body_str = body.unwrap_or("");
            let auth = token
                .map(|t| format!("Authorization: Bearer {t}\r\n"))
                .unwrap_or_default();
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body_str}",
                body_str.len()
            );
            stream.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            buf
        }

        let mut config = Config::default_config();
        config.agents = vec![AgentConfig {
            id: "slowlist".into(),
            name: "SlowList".into(),
            kind: handover_config::AgentKind::Session,
            command: "true".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: Some(10),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: Some(vec!["sleep".into(), "1".into()]),
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        }];
        let daemon = test_daemon(config);
        let token = daemon.api_token.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = api::serve(shared(daemon), 0, Arc::clone(&shutdown)).unwrap();

        let quit_token = token.clone();
        let port = handle.port;
        let sessions_thread =
            std::thread::spawn(move || raw_request(port, "GET", "/sessions", None, Some(&token)));
        // Give the scan a moment to be well underway (subprocess spawned)
        // before measuring the other endpoints.
        std::thread::sleep(std::time::Duration::from_millis(100));

        let start = std::time::Instant::now();
        let status = raw_request(handle.port, "GET", "/status", None, None);
        let status_ms = start.elapsed().as_millis();
        let start = std::time::Instant::now();
        let agents = raw_request(handle.port, "GET", "/agents", None, None);
        let agents_ms = start.elapsed().as_millis();
        assert!(
            status_ms < 900,
            "/status blocked by in-flight /sessions ({status_ms}ms)"
        );
        assert!(
            agents_ms < 900,
            "/agents blocked by in-flight /sessions ({agents_ms}ms)"
        );
        assert!(status.contains("\"daemon\":true"), "status: {status}");
        assert!(agents.contains("slowlist"), "agents: {agents}");
        let _ = sessions_thread.join().unwrap();

        // Authenticated quit for clean shutdown.
        let _ = raw_request(handle.port, "POST", "/quit", Some("{}"), Some(&quit_token));
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    // -- session-aware resolution (Phase 4) --------------------------------

    /// A session-aware test agent config with a temp-dir glob. Uses a neutral
    /// id so discovery exercises the generic id-extraction rule (bare stem);
    /// per-agent rules (codex's `rollout-` prefix, omp's `_` split) are
    /// covered in the core crate's tests.
    fn session_agent(glob: &str) -> AgentConfig {
        AgentConfig {
            id: "testagent".into(),
            name: "TestAgent".into(),
            kind: AgentKind::Session,
            command: "printf 'FRESH:%s' \"{PROMPT}\"".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: Some(10),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some(glob.to_string()),
            session_cli_list: None,
            resume_command: Some("printf 'RESUME:%s SID=%s' \"{PROMPT}\" \"{SESSION}\"".into()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        }
    }

    fn write_session(home: &Path, rel: &str, age_secs: i64) {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        let ts: chrono::DateTime<chrono::Utc> =
            chrono::Utc::now() - chrono::Duration::seconds(age_secs);
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            let times = std::fs::FileTimes::new()
                .set_accessed(ts.into())
                .set_modified(ts.into());
            file.set_times(times).unwrap();
        }
        #[cfg(not(unix))]
        let _ = ts;
    }

    /// Generates a hermes-style session id (`YYYYMMDD_HHMMSS_<hex>`) from a
    /// recent timestamp so tests never go stale as dates pass.
    fn fresh_hermes_id(hours_ago: i64) -> String {
        let dt = chrono::Local::now() - chrono::Duration::hours(hours_ago);
        format!(
            "{}_{}_{:06x}",
            dt.format("%Y%m%d"),
            dt.format("%H%M%S"),
            rand_id()
        )
    }

    fn rand_id() -> u32 {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        // Mask to 24 bits so {:06x} always produces exactly 6 hex digits
        // (matching the hermes session id format: YYYYMMDD_HHMMSS_XXXXXX).
        RandomState::new().build_hasher().finish() as u32 & 0xFFFFFF
    }

    #[test]
    fn live_sessions_scans_glob_freshest_first() {
        let home = std::env::temp_dir().join(format!("ho-sess-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        // Two sessions for the same agent: fresh (5s) and old (1h).
        write_session(&home, "a/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 3600);
        write_session(&home, "b/bbbbbbbb-0000-4000-8000-000000000000.jsonl", 5);

        let mut config = Config::default_config();
        config.agents.push(session_agent(&glob));
        let daemon = test_daemon(config);
        let sessions = daemon.live_sessions();
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0].session_id,
            "bbbbbbbb-0000-4000-8000-000000000000"
        );
        assert_eq!(
            sessions[1].session_id,
            "aaaaaaaa-0000-4000-8000-000000000000"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn resolve_session_order_explicit_then_pinned_then_freshest() {
        let home = std::env::temp_dir().join(format!("ho-res-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "a/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 3600);
        write_session(&home, "b/bbbbbbbb-0000-4000-8000-000000000000.jsonl", 5);

        let mut config = Config::default_config();
        config.agents.push(session_agent(&glob));
        let mut daemon = test_daemon(config);
        let capture = Capture::text(SourceKind::Manual, "session test", None);

        // 3. Freshest wins by default.
        let (_, req) = daemon
            .resolve_send("ask", Some("testagent"), None, capture.clone())
            .expect("resolve");
        assert_eq!(
            req.session.as_ref().unwrap().session_id,
            "bbbbbbbb-0000-4000-8000-000000000000"
        );

        // 2. A pinned session beats freshness (when still live).
        daemon
            .pin_session("testagent", "aaaaaaaa-0000-4000-8000-000000000000")
            .expect("pin");
        let (_, req) = daemon
            .resolve_send("ask", Some("testagent"), None, capture.clone())
            .expect("resolve");
        assert_eq!(
            req.session.as_ref().unwrap().session_id,
            "aaaaaaaa-0000-4000-8000-000000000000"
        );

        // A pinned session that is no longer live falls back to freshest.
        daemon
            .pin_session("testagent", "gone-0000-4000-8000-000000000000")
            .expect("pin");
        let (_, req) = daemon
            .resolve_send("ask", Some("testagent"), None, capture.clone())
            .expect("resolve");
        assert_eq!(
            req.session.as_ref().unwrap().session_id,
            "bbbbbbbb-0000-4000-8000-000000000000"
        );

        // 1. An explicit session always wins.
        let (_, req) = daemon
            .resolve_send(
                "ask",
                Some("testagent"),
                Some("explicit-123"),
                capture.clone(),
            )
            .expect("resolve");
        assert_eq!(req.session.as_ref().unwrap().session_id, "explicit-123");

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn no_live_session_falls_back_to_fresh_send() {
        let home = std::env::temp_dir().join(format!("ho-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());

        let mut config = Config::default_config();
        config.agents.push(session_agent(&glob));
        let daemon = test_daemon(config);
        let capture = Capture::text(SourceKind::Manual, "fresh only", None);
        let (_, req) = daemon
            .resolve_send("ask", Some("testagent"), None, capture)
            .expect("resolve");
        assert!(req.session.is_none(), "no live session → fresh send");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn command_agents_are_unaffected_by_session_resolution() {
        let home = std::env::temp_dir().join(format!("ho-cmd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        write_session(&home, "x/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);

        // demo-echo is a plain Command agent: even with a matching-looking
        // session file on disk, resolution must NOT attach a session.
        let daemon = test_daemon(Config::default_config());
        let capture = Capture::text(SourceKind::Manual, "cmd agent", None);
        let (_, req) = daemon
            .resolve_send("ask", Some("demo-echo"), None, capture)
            .expect("resolve");
        assert!(req.session.is_none());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn send_records_session_id_on_outcome() {
        let home = std::env::temp_dir().join(format!("ho-out-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "b/bbbbbbbb-0000-4000-8000-000000000000.jsonl", 5);

        let mut config = Config::default_config();
        config.agents.push(session_agent(&glob));
        let daemon = test_daemon(config);
        let capture = Capture::text(SourceKind::Manual, "outcome session", None);
        let outcome = daemon
            .send("ask", Some("testagent"), None, capture)
            .expect("send");
        assert!(outcome.ok, "{}", outcome.error.unwrap_or_default());
        assert_eq!(
            outcome.session_id.as_deref(),
            Some("bbbbbbbb-0000-4000-8000-000000000000")
        );
        // The resume command actually ran (SID=<id> echoed by the adapter).
        let stdout = outcome
            .receipt
            .as_ref()
            .and_then(|r| r.stdout.clone())
            .unwrap_or_default();
        assert!(stdout.contains("RESUME:"), "expected resume send: {stdout}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn fresh_send_outcome_has_no_session() {
        let daemon = test_daemon(Config::default_config());
        let capture = Capture::text(SourceKind::Manual, "fresh", None);
        let outcome = daemon.send("ask", None, None, capture).expect("send");
        assert!(outcome.session_id.is_none());
    }

    #[test]
    fn live_owner_refusal_is_humanized() {
        let receipt = handover_core::agent::SendReceipt {
            ok: false,
            agent_id: "hermes".into(),
            agent_name: "Hermes".into(),
            detail: "Exited with status 1".into(),
            stdout: None,
            stderr: Some(
                "Session 20260903_103656_c0f480 already has a live owner \
                 (cli, pid 89301, running 1m). Only 1 service at a time."
                    .into(),
            ),
            duration_ms: 90,
            session_id: Some("20260903_103656_c0f480".into()),
        };
        let msg = humanize_receipt_failure("Hermes", &receipt).expect("humanized");
        assert!(msg.contains("20260903_103656_c0f480"), "{msg}");
        assert!(msg.contains("89301"), "{msg}");
        assert!(!msg.contains("live owner"), "{msg}");
    }

    /// A session-aware agent whose resume refuses with a live-owner error:
    /// auto-resolved sends fall back to fresh; explicit ones fail plainly.
    fn owner_refusing_agent(glob: &str) -> AgentConfig {
        AgentConfig {
            id: "ownertest".into(),
            name: "OwnerTest".into(),
            kind: AgentKind::Session,
            command: "printf 'FRESH-OK'".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: Some(10),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some(glob.to_string()),
            session_cli_list: None,
            resume_command: Some(
                "echo 'Session {SESSION} already has a live owner (cli, pid 42, running 1m)' >&2; exit 1"
                    .into(),
            ),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        }
    }

    #[test]
    fn auto_resolve_falls_back_to_fresh_on_owned_session() {
        let home = std::env::temp_dir().join(format!("ho-own-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "a/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);

        let mut config = Config::default_config();
        config.agents.push(owner_refusing_agent(&glob));
        let daemon = test_daemon(config);
        let capture = Capture::text(SourceKind::Manual, "owned fallback", None);
        let outcome = daemon
            .send("ask", Some("ownertest"), None, capture)
            .expect("send");
        assert!(outcome.ok, "fallback must succeed: {:?}", outcome.error);
        assert!(
            outcome.error.is_none(),
            "no error on fallback success: {:?}",
            outcome.error
        );
        let stdout = outcome
            .receipt
            .as_ref()
            .and_then(|r| r.stdout.clone())
            .unwrap_or_default();
        assert!(stdout.contains("FRESH-OK"), "fresh command ran: {stdout}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn explicit_session_on_owned_session_fails_plainly() {
        let home = std::env::temp_dir().join(format!("ho-ownx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "a/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);

        let mut config = Config::default_config();
        config.agents.push(owner_refusing_agent(&glob));
        let daemon = test_daemon(config);
        let capture = Capture::text(SourceKind::Manual, "owned explicit", None);
        let outcome = daemon
            .send(
                "ask",
                Some("ownertest"),
                Some("aaaaaaaa-0000-4000-8000-000000000000"),
                capture,
            )
            .expect("send");
        assert!(!outcome.ok, "explicit refusal must stay a failure");
        let err = outcome.error.unwrap_or_default();
        assert!(err.contains("already open elsewhere"), "{err}");
        assert!(err.contains("42"), "{err}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn session_state_persists_pins_and_mode_0600() {
        let mut daemon = test_daemon(Config::default_config());
        daemon
            .pin_session("hermes", "20260812_130220_dbf5cf")
            .expect("pin");
        assert_eq!(
            daemon.pinned_session("hermes").as_deref(),
            Some("20260812_130220_dbf5cf")
        );

        // Reload from disk (simulating a restart).
        let state_path = daemon.session_state_path.clone();
        let reloaded = Daemon {
            provider_health: Arc::new(provider_health::ProviderHealth::new()),
            session_state: load_session_state(&state_path),
            ..daemon
        };
        assert_eq!(
            reloaded.pinned_session("hermes").as_deref(),
            Some("20260812_130220_dbf5cf")
        );

        // Unpin through the reloaded instance and verify the file updates.
        let mut reloaded = reloaded;
        reloaded.unpin_session("hermes").expect("unpin");
        assert!(reloaded.pinned_session("hermes").is_none());
        let on_disk = load_session_state(&state_path);
        assert!(on_disk.pinned.is_empty(), "unpin must persist");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "session state must be owner-only");
        }
        if let Some(parent) = state_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn cli_list_sessions_are_discovered_via_command() {
        // Hermes stores sessions in SQLite (no glob), so discovery shells out
        // to `hermes sessions list`. Emulate that with `sh -c` printing a
        // listing — the daemon runs the command, core parses the output.
        let sid = fresh_hermes_id(2);
        let mut config = Config::default_config();
        config.agents.push(AgentConfig {
            id: "hermes".into(),
            name: "Hermes".into(),
            kind: AgentKind::Session,
            command: "hermes chat -q \"{PROMPT}\"".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: Some(10),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: Some(vec![
                "sh".into(),
                "-c".into(),
                format!("printf 'Fake title   neo   8m ago   {sid}\\n'"),
            ]),
            resume_command: Some("hermes chat -q \"{PROMPT}\" --resume {SESSION}".into()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let daemon = test_daemon(config);
        let sessions = daemon.live_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent_id, "hermes");
        assert_eq!(sessions[0].session_id, sid);
        assert!(sessions[0].path.is_none(), "cli-list sessions have no file");

        // Session resolution finds it through the same path.
        let capture = Capture::text(SourceKind::Manual, "cli session", None);
        let (_, req) = daemon
            .resolve_send("ask", Some("hermes"), None, capture)
            .expect("resolve");
        assert_eq!(req.session.as_ref().unwrap().session_id, sid);
    }

    #[test]
    fn cli_list_hangs_fail_soft_instead_of_blocking() {
        // A hung listing (never exits) must not stall discovery: the daemon
        // kills it after the timeout and reports no sessions.
        let mut config = Config::default_config();
        config.agents.push(AgentConfig {
            id: "hangy".into(),
            name: "Hangy".into(),
            kind: AgentKind::Session,
            command: "true".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: Some(vec!["sh".into(), "-c".into(), "sleep 30".into()]),
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let daemon = test_daemon(config);
        let start = std::time::Instant::now();
        let sessions = daemon.live_sessions();
        assert!(sessions.is_empty(), "hung listing must fail soft");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(15),
            "discovery blocked on the hung listing"
        );
    }

    /// Serializes tests that mutate the global `PATH` env var.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn cli_list_program_resolves_bare_names_outside_restricted_path() {
        // The GUI-launched daemon inherits a restricted PATH — a bare
        // `hermes` in `~/.local/bin` must still resolve (this is the bug that
        // made every chat message a fresh send instead of resuming).
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("ho-resolve-{}", uuid::Uuid::new_v4()));
        let bindir = dir.join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        let bin = bindir.join("fake-cli");
        std::fs::write(&bin, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", "/usr/bin:/bin"); // Finder-like restricted PATH
                                                    // Found via PATH (temp dir added to PATH).
        std::env::set_var("PATH", format!("/usr/bin:/bin:{}", bindir.display()));
        assert_eq!(resolve_cli_program("fake-cli"), bin.to_string_lossy());
        // Absolute paths pass through unchanged.
        assert_eq!(
            resolve_cli_program("/opt/weird/bin/tool"),
            "/opt/weird/bin/tool"
        );
        // Not found → falls back to the bare name (Command::new reports it).
        assert_eq!(
            resolve_cli_program("definitely-not-installed"),
            "definitely-not-installed"
        );
        // A non-executable file is skipped, not picked.
        let noexec = bindir.join("noexec");
        std::fs::write(&noexec, "").unwrap();
        std::env::set_var("PATH", format!("/usr/bin:/bin:{}", bindir.display()));
        assert_eq!(resolve_cli_program("noexec"), "noexec");

        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_list_discovery_resolves_bare_program_from_home_bin() {
        // End-to-end: a cli-list agent configured with a bare program name is
        // discovered even when the daemon's PATH cannot see it (the binary
        // lives in ~/.local/bin via the hardcoded fallback dirs).
        let _guard = ENV_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("ho-home-{}", uuid::Uuid::new_v4()));
        let bindir = home.join(".local").join("bin");
        std::fs::create_dir_all(&bindir).unwrap();
        let sid = fresh_hermes_id(2);
        let script = bindir.join("fake-cli");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' 'Title   neo   8m ago   {sid}'\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let old_home = std::env::var_os("HOME");
        let old_path = std::env::var_os("PATH");
        std::env::set_var("HOME", &home);
        std::env::set_var("PATH", "/usr/bin:/bin"); // Finder-like: cannot see ~/.local/bin
        let mut config = Config::default_config();
        config.agents.push(AgentConfig {
            id: "fakec".into(),
            name: "Fake CLI".into(),
            kind: AgentKind::Session,
            command: "true".into(),
            description: None,
            working_dir: None,
            env: Default::default(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: Some(vec!["fake-cli".into()]),
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        });
        let daemon = test_daemon(config);
        let sessions = daemon.live_sessions();
        assert_eq!(
            sessions.len(),
            1,
            "bare cli-list name must resolve via ~/.local/bin"
        );
        assert_eq!(sessions[0].session_id, sid);

        match old_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn sessions_endpoint_requires_auth_and_lists_live() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        fn raw_request(port: u16, path: &str, token: Option<&str>) -> String {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let auth = token
                .map(|t| format!("Authorization: Bearer {t}\r\n"))
                .unwrap_or_default();
            let req = format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\n\r\n"
            );
            stream.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            buf
        }

        let mut config = Config::default_config();
        config.agents.push(session_agent(&format!(
            "{}/*/*.jsonl",
            std::env::temp_dir().join("ho-http-none").display()
        )));
        let daemon = test_daemon(config);
        let token = daemon.api_token.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = api::serve(shared(daemon), 0, Arc::clone(&shutdown)).unwrap();

        // Unauthenticated → 401.
        let denied = raw_request(handle.port, "/sessions", None);
        assert!(
            denied.contains("401") || denied.contains("Unauthorized"),
            "expected unauthorized, got: {denied}"
        );

        // Authenticated → 200 with a JSON array (empty here — no sessions).
        let ok = raw_request(handle.port, "/sessions", Some(&token));
        assert!(ok.contains("200 OK"), "expected 200, got: {ok}");
        let body = ok.split("\r\n\r\n").nth(1).unwrap_or("");
        let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
        assert!(parsed.is_array(), "/sessions must return an array: {body}");

        let _ = raw_request(handle.port, "/quit", Some(&token));
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn sessions_pin_endpoint_pins_freshest_rejects_phantom_and_unpins() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        fn raw_request(
            port: u16,
            method: &str,
            path: &str,
            body: Option<&str>,
            token: Option<&str>,
        ) -> (u16, String) {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let auth = token
                .map(|t| format!("Authorization: Bearer {t}\r\n"))
                .unwrap_or_default();
            let payload = match body {
                Some(b) => format!("Content-Length: {}\r\n\r\n{b}", b.len()),
                None => String::new(),
            };
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n{auth}{payload}"
            );
            stream.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            let status: u16 = buf
                .lines()
                .next()
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            (status, buf)
        }

        let home = std::env::temp_dir().join(format!("ho-pin-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "a/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 3600);
        write_session(&home, "b/bbbbbbbb-0000-4000-8000-000000000000.jsonl", 5);

        let mut config = Config::default_config();
        config.agents.push(session_agent(&glob));
        let daemon = test_daemon(config);
        let token = daemon.api_token.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = api::serve(shared(daemon), 0, Arc::clone(&shutdown)).unwrap();

        // Unauthenticated POST → 401.
        let (status, _) = raw_request(
            handle.port,
            "POST",
            "/sessions/pin",
            Some("{\"agent_id\":\"testagent\"}"),
            None,
        );
        assert_eq!(status, 401, "pin must require auth");

        // Attach flow: pin the freshest live session.
        let (status, body) = raw_request(
            handle.port,
            "POST",
            "/sessions/pin",
            Some("{\"agent_id\":\"testagent\"}"),
            Some(&token),
        );
        assert_eq!(status, 200, "pin failed: {body}");
        let raw = body.split("\r\n\r\n").nth(1).unwrap_or("").trim();
        let parsed: serde_json::Value = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("bad pin response body `{body}` (raw: {raw:?}): {e}"));
        assert_eq!(
            parsed["session_id"], "bbbbbbbb-0000-4000-8000-000000000000",
            "must pin the freshest session: {body}"
        );

        // Explicit pin of a non-live session → 400 (never pin a phantom).
        let (status, body) = raw_request(
            handle.port,
            "POST",
            "/sessions/pin",
            Some("{\"agent_id\":\"testagent\",\"session_id\":\"phantom-0000\"}"),
            Some(&token),
        );
        assert_eq!(status, 400, "phantom pin must be rejected: {body}");
        assert!(body.contains("not live"), "clear error expected: {body}");

        // Unpin → ok.
        let (status, body) = raw_request(
            handle.port,
            "POST",
            "/sessions/unpin",
            Some("{\"agent_id\":\"testagent\"}"),
            Some(&token),
        );
        assert_eq!(status, 200, "unpin failed: {body}");

        let _ = raw_request(handle.port, "POST", "/quit", Some("{}"), Some(&token));
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A session agent with the approval layer fully wired (marker + tmux
    /// channel + target). `target` is the pane spec passed to `tmux
    /// send-keys` — tests stub a fake `tmux` on PATH that appends the
    /// keystroke to $FAKE_TMUX_SINK, mirroring a real injection.
    fn approval_agent(glob: &str, target: &str) -> AgentConfig {
        let mut agent = session_agent(glob);
        agent.permission_marker = Some("[permission]".into());
        agent.approval_channel = Some(handover_config::ApprovalChannel::Tmux);
        agent.approval_target = Some(target.into());
        agent
    }

    /// Installs a fake `tmux` in `bindir` and prepends it to PATH. The stub
    /// appends its key argument to the file named by $FAKE_TMUX_SINK — tests
    /// point that at the transcript so verify-after sees "resumption".
    ///
    /// Mutates global `PATH` — callers must hold [`ENV_LOCK`].
    fn install_fake_tmux(bindir: &std::path::Path, sink: &std::path::Path) {
        std::fs::create_dir_all(bindir).unwrap();
        let tmux = bindir.join("tmux");
        std::fs::write(
            &tmux,
            "#!/bin/sh\n# fake tmux send-keys: append the sent key ($3) to the sink\necho -n \"$3\" >> \"$FAKE_TMUX_SINK\"\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmux, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{old}", bindir.display()));
        std::env::set_var("FAKE_TMUX_SINK", sink);
    }

    #[test]
    fn approve_session_verifies_when_transcript_resumes() {
        // The tmux injection appends the approve keystroke into the
        // transcript: the marker disappears, verify-after confirms.
        let _env = ENV_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("ho-appr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "p/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);
        let transcript = home.join("p/aaaaaaaa-0000-4000-8000-000000000000.jsonl");
        std::fs::write(
            &transcript,
            "assistant: needs input\n[permission] approve shell command?\n",
        )
        .unwrap();

        let mut config = Config::default_config();
        config.agents.push(approval_agent(&glob, "work:0.1"));
        let daemon = test_daemon(config);
        install_fake_tmux(&home.join("bin"), &transcript);

        // Detection: the session is flagged blocked (tail-peek, opt-in) and
        // the marker line is surfaced so the approval card can show what is
        // being asked.
        let sessions = daemon.live_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].blocked, Some(true));
        assert_eq!(
            sessions[0].blocked_detail.as_deref(),
            Some("[permission] approve shell command?")
        );

        let result = daemon
            .approve_session_budget(
                "testagent",
                "aaaaaaaa-0000-4000-8000-000000000000",
                true,
                std::time::Duration::from_secs(3),
            )
            .expect("approve");
        assert!(result.verified, "expected verified: {}", result.message);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn approve_session_fails_soft_when_transcript_does_not_move() {
        // Injection "succeeds" (the fake tmux appends to a dummy sink) but the
        // real transcript never changes → verified=false, never false success.
        let _env = ENV_LOCK.lock().unwrap();
        let home = std::env::temp_dir().join(format!("ho-appr-soft-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "p/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);
        let transcript = home.join("p/aaaaaaaa-0000-4000-8000-000000000000.jsonl");
        std::fs::write(
            &transcript,
            "assistant: needs input\n[permission] approve shell command?\n",
        )
        .unwrap();
        let dummy = home.join("dummy-device");

        let mut config = Config::default_config();
        config.agents.push(approval_agent(&glob, "other:0.2"));
        let daemon = test_daemon(config);
        install_fake_tmux(&home.join("bin"), &dummy);

        let result = daemon
            .approve_session_budget(
                "testagent",
                "aaaaaaaa-0000-4000-8000-000000000000",
                true,
                std::time::Duration::from_millis(400),
            )
            .expect("approve");
        assert!(
            !result.verified,
            "must not claim success without verification: {}",
            result.message
        );
        assert!(
            result.message.contains("couldn't confirm"),
            "fail-soft wording expected: {}",
            result.message
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn approve_session_refuses_when_not_blocked_or_not_opted_in() {
        let home = std::env::temp_dir().join(format!("ho-appr-nb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "p/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);
        let transcript = home.join("p/aaaaaaaa-0000-4000-8000-000000000000.jsonl");
        // No marker: the agent is NOT at a prompt — nothing may be injected.
        std::fs::write(&transcript, "assistant: happily working\n").unwrap();

        let mut config = Config::default_config();
        config
            .agents
            .push(approval_agent(&glob, &transcript.display().to_string()));
        let daemon = test_daemon(config);

        // Not blocked → the re-check refuses to inject.
        let err = daemon
            .approve_session("testagent", "aaaaaaaa-0000-4000-8000-000000000000", true)
            .unwrap_err();
        assert!(
            err.to_string().contains("no longer blocked"),
            "race-safe refusal expected: {err}"
        );

        // Not opted in (no permission_marker) → clear error.
        let mut plain = Config::default_config();
        plain.agents.push(session_agent(&glob));
        let daemon2 = test_daemon(plain);
        let err2 = daemon2
            .approve_session("testagent", "aaaaaaaa-0000-4000-8000-000000000000", true)
            .unwrap_err();
        assert!(err2.to_string().contains("permission_marker"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn sessions_without_marker_config_are_never_peeked() {
        // Privacy: agents without a permission_marker never get a blocked
        // flag — `blocked` stays None, no contents are read.
        let home = std::env::temp_dir().join(format!("ho-appr-np-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "p/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);
        let transcript = home.join("p/aaaaaaaa-0000-4000-8000-000000000000.jsonl");
        std::fs::write(&transcript, "[permission] approve?\n").unwrap();

        let mut config = Config::default_config();
        config.agents.push(session_agent(&glob)); // NO permission_marker
        let daemon = test_daemon(config);
        let sessions = daemon.live_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].blocked, None, "never peek without opt-in");
        assert_eq!(sessions[0].blocked_detail, None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn blocked_detail_is_only_surfaced_when_blocked() {
        // An opted-in agent whose transcript has NO marker is not blocked —
        // the marker line must stay None (no question to show).
        let home = std::env::temp_dir().join(format!("ho-appr-d-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let glob = format!("{}/*/*.jsonl", home.display());
        write_session(&home, "p/aaaaaaaa-0000-4000-8000-000000000000.jsonl", 5);
        let transcript = home.join("p/aaaaaaaa-0000-4000-8000-000000000000.jsonl");
        std::fs::write(&transcript, "assistant: happily working\n").unwrap();

        let mut config = Config::default_config();
        config.agents.push(approval_agent(&glob, "/dev/null"));
        let daemon = test_daemon(config);
        let sessions = daemon.live_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].blocked, Some(false));
        assert_eq!(
            sessions[0].blocked_detail, None,
            "no question when not blocked"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn process_running_reflects_live_processes() {
        // A program that is not running reports false.
        assert!(!agent_process_running("definitely-not-a-real-agent-xyz"));

        // Substring collisions must never light an agent: `omp` inside
        // MTLCompilerService, `codex` inside a longer word or path segment.
        // (Unit-level: the matcher is pure, so these are exact.)
        assert!(!command_line_has_component(
            "/System/Library/MTLCompilerService.xpc/Contents/MacOS/MTLCompilerService -daemon",
            "omp"
        ));
        assert!(!command_line_has_component(
            "npm exec vite --port 5199 --strictPort",
            "codex"
        ));
        assert!(command_line_has_component(
            "/bin/sh -c /Users/x/tools/omp -p \"hi\"",
            "omp"
        ));
        assert!(!command_line_has_component(
            "tail -f /var/log/omp.log",
            "omp"
        ));
        assert!(!command_line_has_component("", "omp"));
        assert!(!command_line_has_component("anything", ""));

        // Spawn a real child process with a distinctive program name, then
        // verify the check sees it while alive and loses it after exit.
        // (We must NOT probe the test binary itself: macOS `pgrep -f` never
        // matches its own parent process, so a self-check would false-fail
        // even though production checks always target OTHER processes.)
        #[cfg(unix)]
        {
            let probe = std::env::temp_dir().join(format!("ho-probe-{}", uuid::Uuid::new_v4()));
            std::fs::copy("/bin/sleep", &probe).expect("copy sleep for probe");
            let child = std::process::Command::new(&probe)
                .arg("30")
                .spawn()
                .expect("spawn probe");
            assert!(
                agent_process_running(&probe.to_string_lossy()),
                "a live child process must be seen as running"
            );
            // Kill the child, then it must read as not running.
            let mut waiter = child;
            waiter.kill().expect("kill probe");
            waiter.wait().expect("probe reaped");
            assert!(
                !agent_process_running(&probe.to_string_lossy()),
                "a dead process must not be seen as running"
            );
            let _ = std::fs::remove_file(&probe);
        }
    }

    #[test]
    fn agents_status_marks_running_from_config_command() {
        // Point an agent at a spawned child process — the status light must
        // see it as online while alive and offline after it exits. (The
        // agent command must target a real OTHER process: macOS `pgrep -f`
        // never matches the caller's own parent, so pointing it at the test
        // binary itself would false-fail.)
        #[cfg(unix)]
        {
            let probe = std::env::temp_dir().join(format!("ho-probe-{}", uuid::Uuid::new_v4()));
            std::fs::copy("/bin/sleep", &probe).expect("copy sleep for probe");
            let child = std::process::Command::new(&probe)
                .arg("30")
                .spawn()
                .expect("spawn probe");
            let mut config = Config::default_config();
            config.agents.push(AgentConfig {
                id: "probe".into(),
                name: "Probe Agent".into(),
                kind: AgentKind::Command,
                command: format!("{} --probe", probe.display()),
                description: None,
                working_dir: None,
                env: Default::default(),
                timeout_secs: None,
                enabled: true,
                demo: false,
                default_action: None,
                session_glob: None,
                session_cli_list: None,
                resume_command: None,
                permission_marker: None,
                approval_channel: None,
                approval_target: None,
            });
            let daemon = test_daemon(config);
            let statuses = daemon.agents_status();
            let s = statuses
                .iter()
                .find(|s| s.meta.id == "probe")
                .expect("agent present");
            assert!(s.status.available, "{}", s.status.detail);
            assert!(s.status.running, "live child must read as online");
            let mut waiter = child;
            waiter.kill().expect("kill probe");
            waiter.wait().expect("probe reaped");
            let statuses = daemon.agents_status();
            let s = statuses
                .iter()
                .find(|s| s.meta.id == "probe")
                .expect("agent present");
            assert!(!s.status.running, "dead child must read as offline");
            let _ = std::fs::remove_file(&probe);
        }
    }

    #[test]
    fn humanize_never_leaks_internals() {
        let err = AgentError::Unavailable {
            agent: "hermes".into(),
            detail: "Command `hermes` not found on PATH".into(),
        };
        let msg = humanize_agent_error(&err);
        assert!(msg.contains("hermes"));
        assert!(msg.contains("not available"));
        assert!(!msg.contains("AgentError"));
    }

    #[test]
    fn redact_command_summary_is_opaque() {
        assert_eq!(
            redact_command_summary(r#"hermes -z "secret-token-xyz""#),
            "configured command"
        );
        assert_eq!(
            redact_command_summary("API_KEY=secret my-agent --prompt x"),
            "configured command"
        );
        assert_eq!(redact_command_summary("cat"), "configured command");
    }

    #[test]
    fn oversized_text_is_truncated_with_note() {
        let daemon = test_daemon(Config::default_config());
        // Just over the text cap.
        let big = "x".repeat(MAX_TEXT_CAPTURE_BYTES + 64);
        let capture = Capture::text(SourceKind::Clipboard, big, None);
        let prompt = daemon
            .render_prompt("ask", capture)
            .expect("render should succeed");
        assert!(
            prompt.contains("Text note:"),
            "text_note missing from prompt: {prompt}"
        );
        assert!(prompt.contains("truncated"));
        // Body should not retain the full original size.
        assert!(
            prompt.len() < MAX_TEXT_CAPTURE_BYTES + 2_000,
            "prompt still too large: {}",
            prompt.len()
        );
    }

    #[test]
    fn status_reports_max_agent_timeout() {
        let mut config = Config::default_config();
        config.agents[0].timeout_secs = Some(300);
        let daemon = test_daemon(config);
        let status = daemon.status();
        assert_eq!(status["max_agent_timeout_secs"], 300);
    }

    #[test]
    fn read_file_nofollow_refuses_symlink() {
        let dir = std::env::temp_dir().join(format!("ho-nofollow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("secret.env");
        std::fs::write(&secret, "SECRET=1\n").unwrap();
        let link = dir.join("looks-safe.log");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&secret, &link).unwrap();
            let err = read_file_nofollow(&link, MAX_FILE_CAPTURE_BYTES).unwrap_err();
            assert!(
                err.contains("symbolic link") || err.contains("Refusing"),
                "expected symlink refusal, got: {err}"
            );
        }
        #[cfg(not(unix))]
        {
            let _ = (secret, link);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_file_nofollow_reads_regular_file() {
        let dir = std::env::temp_dir().join(format!("ad-read-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ok.txt");
        std::fs::write(&path, "hello nofollow").unwrap();
        let (meta, bytes) = read_file_nofollow(&path, MAX_FILE_CAPTURE_BYTES).unwrap();
        assert!(meta.is_file());
        assert_eq!(bytes.as_deref(), Some(b"hello nofollow".as_slice()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_command_bounded_kills_child_at_deadline() {
        let start = std::time::Instant::now();
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let out = run_command_bounded(&mut cmd, std::time::Duration::from_millis(100));
        assert!(out.is_none(), "hung child must return None at the deadline");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "bound must fire near the budget, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn run_command_bounded_returns_output_on_success() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "echo hi; echo oops 1>&2"]);
        let out = run_command_bounded(&mut cmd, std::time::Duration::from_secs(5))
            .expect("quick child must succeed");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "oops");
    }
}
