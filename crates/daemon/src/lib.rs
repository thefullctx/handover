//! Handover daemon service.
//!
//! The daemon owns the configuration and the agent registry, normalizes
//! captures (dropped text/files, CLI args, stdin — the clipboard is never
//! read), renders prompts and executes handoffs. It exposes the same logic
//! through two clients:
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

mod history;
mod process;
mod runner;
mod send;
mod session_state;
mod status;

pub use approval::{complete_approval, execute_approval, ApprovalPlan, ApprovalRequest};
pub use capture::{read_file_nofollow, MAX_FILE_CAPTURE_BYTES, MAX_TEXT_CAPTURE_BYTES};
pub use history::{next_handoff_id, HandoffHistory, SendOutcome, HANDOFF_HISTORY_CAP};
pub use send::{complete_send, execute_handoff, humanize_agent_error, SendPlan};
pub use session_state::SessionState;
pub use status::{
    compute_agents_status, compute_live_sessions, AgentsStatusSnapshot, LiveSessionsSnapshot,
};

pub(crate) use capture::cap_text_capture;
pub(crate) use runner::run_command_bounded;
pub(crate) use session_state::{load_session_state, save_session_state, session_state_path_for};

// The items below are re-exported for the in-crate tests module (reached via
// `super::*`); no non-test code in the crate needs the crate-root path.
#[cfg(test)]
pub(crate) use process::{agent_process_running, command_line_has_component};
#[cfg(test)]
pub(crate) use runner::resolve_cli_program;
#[cfg(test)]
pub(crate) use send::humanize_receipt_failure;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use handover_agents::AgentRegistry;
use handover_config::{
    builtin_session_agents, AgentConfig, AgentKind, Appearance, Config, ConfigError,
};
use handover_core::action::{builtin_actions, find_action, Action};
use handover_core::agent::{AgentError, AgentMetaStatus, AgentRequest};
use handover_core::capture::{Capture, ContentKind};
use handover_core::exclusions::is_excluded_path;
use handover_core::prompt::render_action_prompt;
use handover_core::session::{LiveSession, SessionSpec};

pub const DEFAULT_PORT: u16 = 47444;

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

/// Shared, mutable daemon state managed by the GUI / HTTP server.
#[derive(Clone)]
pub struct SharedDaemon(pub Arc<Mutex<Daemon>>);

impl SharedDaemon {
    /// Locks the daemon, recovering from a poisoned mutex.
    ///
    /// A panic while the lock is held poisons it for the rest of the process:
    /// every later `lock().unwrap()` panics too, so one bad thread would brick
    /// the HTTP API and the palette rather than failing loudly once. Nothing
    /// in `Daemon` holds a cross-field invariant that a mid-mutation panic
    /// could corrupt — every persisted write is atomic on disk — so recovering
    /// (and logging loudly) beats taking the whole app down with it.
    ///
    /// Every API handler and Tauri command goes through this instead of
    /// calling `.0.lock()` directly, so poison handling is uniform.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, Daemon> {
        self.0.lock().unwrap_or_else(|poisoned| {
            log::error!("daemon state was poisoned by an earlier panic; recovering");
            poisoned.into_inner()
        })
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
        // Write paths are fail-loud: an invalid config is rejected at the
        // boundary instead of being persisted (and then silently skipped on
        // the next registry rebuild).
        agent.validate().map_err(DaemonError::Message)?;
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
    ///
    /// The privacy exclusions run for ANY path-carrying capture (path-only
    /// checks, no file opened) — an image capture pointing at `.env` must be
    /// refused just like a file capture.
    ///
    /// Only `File` captures are READ and embedded, though. Image captures
    /// hand off by PATH BY DESIGN (the agent opens the file itself) — reading
    /// the bytes just to discover they are binary would stamp every image
    /// handoff with a misleading `file_note` ("binary file — contents not
    /// embedded") and waste an up-to-1 MiB read.
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

        if capture.content.kind != ContentKind::File {
            return Ok(());
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

pub fn port_from_env() -> u16 {
    std::env::var("HANDOVER_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
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

#[cfg(test)]
mod tests {
    /// Spawns a live child process under a distinctive, deliberately LONG
    /// name (well past Linux's 15-char `comm` limit) and returns it.
    ///
    /// It has to be a script, not a copied binary. On Ubuntu 26.04 `/bin/sleep`
    /// resolves to `/usr/lib/cargo/bin/coreutils/sleep` — a coreutils multicall
    /// binary that dispatches on `argv[0]` — so a copy under another name exits
    /// immediately with "unknown program". A dead probe reads as "not running",
    /// which failed these tests on 26.04 without any of the detection code
    /// having changed.
    ///
    /// The long name is the point: `pgrep -x` compares against `comm`, which
    /// the kernel truncates, so these probes are found by the `ps` command-line
    /// scan — the fallback path production actually depends on.
    #[cfg(unix)]
    fn spawn_probe() -> (std::path::PathBuf, std::process::Child) {
        let probe = std::env::temp_dir().join(format!("ho-probe-{}", uuid::Uuid::new_v4()));
        std::fs::write(&probe, "#!/bin/sh\nsleep 30\n").expect("write probe script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&probe).expect("stat probe").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&probe, perms).expect("chmod probe");
        }
        let child = std::process::Command::new(&probe)
            .spawn()
            .expect("spawn probe");
        (probe, child)
    }
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
    fn add_agent_rejects_invalid_config_before_persisting() {
        // Write paths are fail-loud: a session agent whose resume_command
        // misses {SESSION} is rejected at the boundary — never persisted,
        // never silently skipped by a later registry rebuild.
        let mut daemon = test_daemon(Config::default_config());
        let mut invalid = AgentConfig {
            id: "broken".into(),
            name: "Broken".into(),
            kind: AgentKind::Session,
            command: "agent --prompt \"{PROMPT}\"".into(),
            description: None,
            working_dir: None,
            env: std::collections::HashMap::new(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.broken/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some("agent --prompt \"{PROMPT}\"".into()), // missing {SESSION}
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        };
        let err = daemon.add_agent(invalid.clone()).unwrap_err();
        assert!(
            err.to_string().contains("{SESSION}"),
            "rejection must mention {{SESSION}}: {err}"
        );
        assert!(
            daemon.configured_agents().iter().all(|a| a.id != "broken"),
            "rejected config must not be persisted"
        );

        // Fixing resume_command makes the same agent acceptable.
        invalid.resume_command = Some("agent --resume {SESSION} --prompt \"{PROMPT}\"".into());
        daemon.add_agent(invalid).expect("valid config accepted");
        assert!(daemon.registry.get("broken").is_some());
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

    /// Serializes tests that mutate process-global env vars (PATH, HOME,
    /// FAKE_TMUX_SINK). Only tests that MUTATE the environment take this
    /// lock; everything else runs freely in parallel.
    ///
    /// Acquired poison-tolerant on purpose: a panic inside a holder must not
    /// fail every later env-dependent test. The real hazard a panic leaves
    /// behind is the MUTATED environment, and that is handled by [`EnvGuard`]
    /// (restore-on-Drop runs during unwinding).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Acquires [`ENV_LOCK`], tolerating poison (see the lock's doc).
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Captures process-global env vars and restores them on Drop.
    ///
    /// Env-mutating tests must hold a guard so the original values come back
    /// even when the test PANICS mid-body (Drop runs during unwinding). The
    /// old restore-at-the-end pattern leaked a mutated environment on panic:
    /// the next env-dependent test then failed (e.g. PATH pointing at a
    /// deleted temp dir, HOME at a removed home), panicked in turn, and the
    /// cascade spread PoisonErrors to every serialized test.
    struct EnvGuard {
        path: Option<std::ffi::OsString>,
        home: Option<std::ffi::OsString>,
        fake_tmux_sink: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn capture() -> Self {
            Self {
                path: std::env::var_os("PATH"),
                home: std::env::var_os("HOME"),
                fake_tmux_sink: std::env::var_os("FAKE_TMUX_SINK"),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            let restore = |key: &str, val: &Option<std::ffi::OsString>| match val {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            };
            restore("PATH", &self.path);
            restore("HOME", &self.home);
            restore("FAKE_TMUX_SINK", &self.fake_tmux_sink);
        }
    }

    #[test]
    fn cli_list_program_resolves_bare_names_outside_restricted_path() {
        // The GUI-launched daemon inherits a restricted PATH — a bare
        // `hermes` in `~/.local/bin` must still resolve (this is the bug that
        // made every chat message a fresh send instead of resuming).
        let _lock = lock_env();
        let _env = EnvGuard::capture();
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

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cli_list_discovery_resolves_bare_program_from_home_bin() {
        // End-to-end: a cli-list agent configured with a bare program name is
        // discovered even when the daemon's PATH cannot see it (the binary
        // lives in ~/.local/bin via the hardcoded fallback dirs).
        let _lock = lock_env();
        let _env = EnvGuard::capture();
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

        // Restore HOME/PATH BEFORE deleting the temp home: other tests run
        // concurrently (they do not take the env lock) and a demo-agent send
        // resolving `~/.handover/demo-handoff.txt` against a directory that
        // is being removed would fail its handoff.
        drop(_env);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn concurrent_burst_never_hangs_or_exhausts_threads() {
        // Regression for the in-flight request cap: an unauthenticated burst
        // (a webpage firing loopback GETs at 127.0.0.1, say) must never grow
        // threads without bound or stall the daemon. Every response is a
        // prompt 200 or the bounded 503 — never a hang.
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

        let daemon = test_daemon(Config::default_config());
        let token = daemon.api_token.clone();
        let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let handle = api::serve(shared(daemon), 0, Arc::clone(&shutdown)).unwrap();
        let port = handle.port;

        // 48 concurrent hits on the (unauthenticated) /agents status route,
        // which spawns per-agent pgrep/ps + provider probes per call.
        let workers: Vec<_> = (0..48)
            .map(|_| {
                let token = token.clone();
                std::thread::spawn(move || raw_request(port, "GET", "/agents", None, Some(&token)))
            })
            .collect();
        let start = std::time::Instant::now();
        for w in workers {
            let resp = w.join().unwrap();
            assert!(
                resp.contains("200 OK") || resp.contains("503"),
                "unexpected status in burst response: {resp}"
            );
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(20),
            "burst must drain quickly, took {:?}",
            start.elapsed()
        );

        // The daemon is still alive and serving after the burst.
        let health = raw_request(port, "GET", "/health", None, None);
        assert!(health.contains("\"ok\":true"), "health: {health}");

        let _ = raw_request(port, "POST", "/quit", Some("{}"), Some(&token));
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
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
    /// Mutates global `PATH` + `FAKE_TMUX_SINK` — callers must hold the env
    /// lock (`lock_env`) and keep an [`EnvGuard`] so both are restored when
    /// the test ends (or panics).
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
        let _lock = lock_env();
        let _env = EnvGuard::capture(); // install_fake_tmux mutates PATH/FAKE_TMUX_SINK
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
        let _lock = lock_env();
        let _env = EnvGuard::capture(); // install_fake_tmux mutates PATH/FAKE_TMUX_SINK
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

    /// Polls [`agent_process_running`] until it sees the probe or the budget
    /// expires. `Command::spawn` returns before the child's exec completes, so
    /// a single check right after `spawn()` can race the exec (the process
    /// still carries its parent's image name) and read false under load.
    fn wait_until_running(probe: &str) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if agent_process_running(probe) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
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

        // Regression: shell machinery that first_program can fail to see
        // through must NEVER be pgrep'd — `env` runs in countless unrelated
        // shell scripts, so `pgrep -x env` would light agents green that are
        // not running at all.
        assert!(!agent_process_running("env"));
        assert!(!agent_process_running("sh"));
        assert!(!agent_process_running("sh -c ''"));

        // Spawn a real child process with a distinctive program name, then
        // verify the check sees it while alive and loses it after exit.
        // (We must NOT probe the test binary itself: macOS `pgrep -f` never
        // matches its own parent process, so a self-check would false-fail
        // even though production checks always target OTHER processes.)
        #[cfg(unix)]
        {
            let (probe, child) = spawn_probe();
            assert!(
                wait_until_running(&probe.to_string_lossy()),
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
            let (probe, child) = spawn_probe();
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
            // Poll: spawn() returns before exec completes — a single status
            // read can race the exec and miss the running process.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let (available, running) = loop {
                let statuses = daemon.agents_status();
                let s = statuses
                    .iter()
                    .find(|s| s.meta.id == "probe")
                    .expect("agent present");
                if s.status.running || std::time::Instant::now() >= deadline {
                    break (s.status.available, s.status.running);
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            };
            assert!(available, "probe agent must be available");
            assert!(running, "live child must read as online");
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
    fn image_capture_is_path_only_no_binary_note() {
        // Images hand off by PATH BY DESIGN: enrichment must not read the
        // bytes (they are binary) and must not stamp the prompt with a
        // misleading "binary file — contents not embedded" / "not attached"
        // note. Regression: image captures used to fall through the same
        // file-enrichment path as text files.
        let dir = std::env::temp_dir().join(format!("ho-img-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // A tiny real PNG header: binary (NUL bytes), under the size cap.
        let png: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        let path = dir.join("shot.png");
        std::fs::write(&path, &png).unwrap();

        let daemon = test_daemon(Config::default_config());
        let capture = Capture::image(path.to_string_lossy().to_string());
        let prompt = daemon
            .render_prompt("ask", capture)
            .expect("render should succeed");
        assert!(
            prompt.contains("Attached image: ") && prompt.contains("shot.png"),
            "image must be attached by path: {prompt}"
        );
        assert!(
            !prompt.contains("File note") && !prompt.contains("binary"),
            "image must not carry a binary/file-note: {prompt}"
        );

        // The privacy exclusions still apply to image captures (path-only
        // check — an image "pointing" at a secret is refused, not attached
        // by path).
        let excluded = Capture::image("/tmp/project/.env".to_string());
        let err = daemon
            .render_prompt("ask", excluded)
            .expect_err("excluded image path must be refused");
        assert!(
            err.to_string().contains("Refusing"),
            "image to an excluded path must be refused: {err}"
        );
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

    #[test]
    fn run_command_bounded_caps_buffered_output() {
        // A discovery command that prints far more than any caller needs must
        // not be able to grow the daemon's memory without bound. The child is
        // still drained to EOF (so it never blocks on a full pipe) — only the
        // retained bytes are capped.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "i=0; while [ $i -lt 20000 ]; do printf 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\n'; i=$((i+1)); done"]);
        let out = run_command_bounded(&mut cmd, std::time::Duration::from_secs(20))
            .expect("child must finish");
        assert!(out.status.success());
        assert!(
            out.stdout.len() <= 256 * 1024,
            "buffered output must stay capped, got {} bytes",
            out.stdout.len()
        );
        // ...and it really did read more than the cap, so the cap is what
        // stopped it (not a child that happened to print very little).
        assert!(
            out.stdout.len() > 200 * 1024,
            "fixture should exceed the cap"
        );
    }

    #[test]
    fn flag_like_program_name_is_never_pg_repd() {
        // `first_program` can return a name that starts with `-` from a
        // hand-edited agent command. Passing that straight to `pgrep` would be
        // parsed as a FLAG (`-f` matches every process) and light every agent
        // green. It must fail closed instead.
        assert!(!agent_process_running("-f"));
        assert!(!agent_process_running("--help"));
    }

    #[test]
    fn unknown_route_answers_404_not_400() {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::atomic::AtomicBool;

        fn raw_request(port: u16, method: &str, path: &str, token: Option<&str>) -> String {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let auth = token
                .map(|t| format!("Authorization: Bearer {t}\r\n"))
                .unwrap_or_default();
            let req = format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\n\r\n"
            );
            stream.write_all(req.as_bytes()).unwrap();
            let mut buf = String::new();
            stream.read_to_string(&mut buf).unwrap();
            buf
        }

        let daemon = test_daemon(Config::default_config());
        let token = daemon.api_token.clone();
        let handle = api::serve(shared(daemon), 0, Arc::new(AtomicBool::new(false))).unwrap();

        let resp = raw_request(handle.port, "GET", "/definitely-not-a-route", Some(&token));
        assert!(
            resp.starts_with("HTTP/1.1 404"),
            "unknown route must be 404: {resp}"
        );
        // A real route with a bad request keeps 400.
        let resp = raw_request(handle.port, "POST", "/preferences", Some(&token));
        assert!(
            resp.starts_with("HTTP/1.1 400"),
            "malformed body must be 400: {resp}"
        );
    }

    #[test]
    fn mutating_routes_reject_requests_without_the_token() {
        // Only `/sessions` GET had an auth test; the routes that can actually
        // trigger agent work or stop the daemon must be covered too — an
        // unauthenticated local process must never be able to drive them.
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::atomic::{AtomicBool, Ordering};

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

        let daemon = test_daemon(Config::default_config());
        let token = daemon.api_token.clone();
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = api::serve(shared(daemon), 0, Arc::clone(&shutdown)).unwrap();

        let send_body = r#"{"action_id":"ask","capture":{"source":{"type":"manual"},"content":{"type":"text","text":"hi"}}}"#;
        for (method, path, body) in [
            ("POST", "/send", Some(send_body)),
            ("POST", "/render", Some(send_body)),
            ("POST", "/quit", Some("{}")),
            (
                "POST",
                "/preferences",
                Some(r#"{"action_id":"ask","agent_id":"demo-echo"}"#),
            ),
        ] {
            let anon = raw_request(handle.port, method, path, body, None);
            assert!(
                anon.starts_with("HTTP/1.1 401"),
                "{method} {path} must reject an unauthenticated request: {anon}"
            );
            assert!(
                !shutdown.load(Ordering::Relaxed),
                "{method} {path} must not take effect without the token"
            );
        }

        // The same routes DO work once the token is presented.
        let authed = raw_request(handle.port, "POST", "/quit", Some("{}"), Some(&token));
        assert!(
            authed.starts_with("HTTP/1.1 200"),
            "authenticated POST /quit must succeed: {authed}"
        );
        assert!(shutdown.load(Ordering::Relaxed));
    }

    #[test]
    fn sessions_pin_never_holds_the_lock_across_the_session_scan() {
        // Regression: POST /sessions/pin used to call `compute_live_sessions`
        // while still holding the daemon mutex, so a slow cli-list (5s budget
        // per agent) froze /health, /status, the palette and every in-flight
        // handoff. The scan must happen off the lock, like every other route.
        use std::io::{Read, Write};
        use std::net::TcpStream;
        use std::sync::atomic::AtomicBool;

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
        // The cli-list drops a marker as soon as the scan starts, then keeps
        // running — so the test can observe "the scan is in flight right now"
        // deterministically instead of racing the request thread.
        let marker = std::env::temp_dir().join(format!(
            "ho-pin-scan-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let marker_arg = marker.display().to_string();
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
            session_cli_list: Some(vec![
                "sh".into(),
                "-c".into(),
                format!("touch '{marker_arg}'; sleep 3"),
            ]),
            resume_command: Some("true --resume {SESSION}".into()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        }];
        let daemon = test_daemon(config);
        let token = daemon.api_token.clone();
        let handle = api::serve(shared(daemon), 0, Arc::new(AtomicBool::new(false))).unwrap();

        // Fire the slow pin on its own thread...
        let pin = {
            let token = token.clone();
            std::thread::spawn(move || {
                raw_request(
                    handle.port,
                    "POST",
                    "/sessions/pin",
                    Some(r#"{"agent_id":"slowlist"}"#),
                    Some(&token),
                )
            })
        };

        // ...and wait until its cli-list is demonstrably mid-scan. If the route
        // held the daemon lock across that scan, the lock is held RIGHT NOW.
        let scan_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < scan_deadline,
                "the pin's cli-list never started — the fixture is wrong"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let started = std::time::Instant::now();
        let status = raw_request(handle.port, "GET", "/status", None, None);
        let elapsed = started.elapsed();
        assert!(
            status.starts_with("HTTP/1.1 200"),
            "/status must answer during the scan: {status}"
        );
        assert!(
            elapsed < std::time::Duration::from_millis(1000),
            "/status waited {elapsed:?} behind the pin's session scan — the daemon \
             lock is being held across the scan again"
        );
        let _ = std::fs::remove_file(&marker);

        // The pin itself still completes (it just doesn't hold the lock).
        let pin_response = pin.join().expect("pin thread");
        assert!(
            pin_response.starts_with("HTTP/1.1 400"),
            "pinning an agent with no live session fails cleanly: {pin_response}"
        );
    }
}
