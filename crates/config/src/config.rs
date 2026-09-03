use crate::config::ConfigError::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// What kind of integration an agent uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// Invoke a user-configured shell command, e.g.
    /// `hermes --prompt "{PROMPT}"` — supports any agent.
    #[default]
    Command,
    /// A generic OpenAI-compatible HTTP endpoint (planned; not in the slice).
    OpenAiCompatible,
    /// A session-aware agent: knows about the user's live sessions and can
    /// resume into them by session id (see `session_glob`, `resume_command`).
    /// Behaves exactly like [`AgentKind::Command`] for fresh sends; the
    /// session-aware adapter (Phase 3) adds resume-by-id.
    Session,
}

/// How a session-aware agent can be approved mid-run, when it is blocked on
/// a permission prompt. Opt-in and per-agent; `None` means approvals are
/// handled in the agent's own UI only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChannel {
    /// Write a single keystroke to the session's tty device.
    Tty,
    /// Send a keystroke via `tmux send-keys`.
    Tmux,
    /// Use an agent-specific external channel (permission file, IPC, …)
    /// when the agent exposes one.
    Agent,
}

/// How the UI follows the system appearance (palette + Settings windows).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follow the OS light/dark appearance.
    #[default]
    System,
    /// Always use the light appearance.
    Light,
    /// Always use the dark appearance.
    Dark,
}

impl Appearance {
    /// Stable string form (`system` | `light` | `dark`) for IPC with the UI.
    pub fn as_str(&self) -> &'static str {
        match self {
            Appearance::System => "system",
            Appearance::Light => "light",
            Appearance::Dark => "dark",
        }
    }
}

impl std::str::FromStr for Appearance {
    type Err = String;

    /// Inverse of [`Appearance::as_str`] — the single source for the
    /// value↔enum mapping, so the command layer cannot drift from it.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Appearance::System),
            "light" => Ok(Appearance::Light),
            "dark" => Ok(Appearance::Dark),
            other => Err(format!(
                "invalid appearance `{other}` (expected `system`, `light` or `dark`)"
            )),
        }
    }
}

/// A single configured agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub id: String,
    pub name: String,
    pub kind: AgentKind,
    /// Shell command. `{PROMPT}` substitutes the rendered prompt,
    /// `{PROMPT_FILE}` substitutes a temp file path. If neither is present
    /// the prompt is piped to the command on stdin.
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub demo: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_action: Option<String>,
    /// Session-aware agents only. Glob (home-relative, `~` optional) over
    /// the agent's session transcript files; the filename is the session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_glob: Option<String>,
    /// Session-aware agents only. Command + args whose stdout lists sessions
    /// (used when the agent has no filesystem glob, e.g. Hermes' SQLite
    /// store → `hermes sessions list`). Mutually exclusive with `session_glob`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_cli_list: Option<Vec<String>>,
    /// Session-aware agents only. Command run when sending into an existing
    /// session; must contain `{SESSION}` (substituted with the session id)
    /// and may use `{PROMPT}` / `{PROMPT_FILE}` like `command`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_command: Option<String>,
    /// Approval layer (Phase 6.5, opt-in). Tail-peek pattern that marks a
    /// session as blocked waiting for approval. When set, discovery may read
    /// ONLY the last few KB of a live session's transcript looking for this
    /// marker — the single documented privacy-rule exception.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_marker: Option<String>,
    /// Approval layer (Phase 6.5, opt-in). How an approve/deny decision is
    /// delivered back into the session. `None` = approvals handled in the
    /// agent's own UI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_channel: Option<ApprovalChannel>,
    /// Approval layer: the channel's target — a tty device path (e.g.
    /// `/dev/ttys002`) for `tty`, a pane/target for `tmux`. Recorded by
    /// `handover attach` (run inside the session) or set in config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_target: Option<String>,
}

impl AgentConfig {
    /// Validates an agent config. Session-aware fields are validated only when
    /// present (additive schema — a plain command agent is always valid).
    pub fn validate(&self) -> Result<(), String> {
        if let Some(resume) = &self.resume_command {
            if !resume.contains("{SESSION}") {
                return Err(format!(
                    "Agent `{}`: resume_command must contain {{SESSION}} to target a live session.",
                    self.id
                ));
            }
        }
        if self.session_glob.is_some() && self.session_cli_list.is_some() {
            return Err(format!(
                "Agent `{}`: session_glob and session_cli_list are mutually exclusive.",
                self.id
            ));
        }
        if self.kind == AgentKind::Session
            && self.session_glob.is_none()
            && self.session_cli_list.is_none()
        {
            return Err(format!(
                "Agent `{}`: a session-aware agent needs session_glob or session_cli_list.",
                self.id
            ));
        }
        Ok(())
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// e.g. "CmdOrCtrl+Shift+A" (informational; the desktop app registers it).
    #[serde(default = "default_shortcut")]
    pub shortcut: String,
    #[serde(default)]
    pub launch_at_startup: bool,
    /// Skip the preview / agent picker when a preferred agent is known.
    #[serde(default = "default_true")]
    pub quick_send: bool,
    /// Show desktop notifications for handoff milestones (sent / completed / failed).
    #[serde(default = "default_true")]
    pub notifications: bool,
    #[serde(default = "default_retention")]
    pub history_retention_days: u32,
    /// UI appearance: follow the system, or force light/dark.
    #[serde(default)]
    pub appearance: Appearance,
    /// Window surface opacity for palette + Settings (0.4–1.0). Applies to
    /// glass fills so both windows stay in sync; default fully opaque.
    #[serde(default = "default_ui_opacity")]
    pub ui_opacity: f64,
}

fn default_shortcut() -> String {
    "CmdOrCtrl+Shift+A".to_string()
}
fn default_retention() -> u32 {
    7
}
fn default_ui_opacity() -> f64 {
    1.0
}

/// Clamps opacity into the supported range (readable minimum → fully solid).
pub fn clamp_ui_opacity(value: f64) -> f64 {
    if !value.is_finite() {
        return default_ui_opacity();
    }
    value.clamp(0.4, 1.0)
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            shortcut: default_shortcut(),
            launch_at_startup: false,
            quick_send: true,
            notifications: true,
            history_retention_days: default_retention(),
            appearance: Appearance::System,
            ui_opacity: default_ui_opacity(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Glob patterns for files that must never be attached.
    #[serde(default = "crate::config::default_exclusions")]
    pub excluded_paths: Vec<String>,
    #[serde(default)]
    pub excluded_apps: Vec<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            excluded_paths: default_exclusions(),
            excluded_apps: vec![],
        }
    }
}

/// The full Handover configuration, persisted as TOML under the platform
/// config directory (see [`Config::config_path`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub agents: Vec<AgentConfig>,
    /// Maps action id -> preferred agent id.
    #[serde(default)]
    pub preferences: HashMap<String, String>,
    #[serde(default)]
    pub privacy: PrivacyConfig,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config error: {0}")]
    Message(String),
    #[error("could not read config: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not parse config: {0}")]
    Parse(#[from] toml::de::Error),
}

impl Default for Config {
    fn default() -> Self {
        Self::default_config()
    }
}

impl Config {
    /// The default configuration: a single demo "echo" agent that records
    /// handoffs to a local file so the vertical slice can be verified
    /// end-to-end without a real agent installed.
    pub fn default_config() -> Self {
        Self {
            general: GeneralConfig {
                shortcut: default_shortcut(),
                launch_at_startup: false,
                quick_send: true,
                notifications: true,
                history_retention_days: default_retention(),
                appearance: Appearance::System,
                ui_opacity: default_ui_opacity(),
            },
            agents: vec![AgentConfig {
                id: "demo-echo".to_string(),
                name: "Echo (demo)".to_string(),
                kind: AgentKind::Command,
                // Sink lives next to the config in the user's own app dir
                // (owner-only), never world-readable /tmp.
                command: "sh -c 'tee \"$HANDOVER_DEMO_SINK\"'".to_string(),
                description: Some(
                    "Echoes the prompt back into a private file next to your config. ".to_string()
                        + "Use it to verify Handover works, then replace it with a real agent.",
                ),
                working_dir: None,
                env: HashMap::new(),
                timeout_secs: Some(60),
                enabled: true,
                demo: true,
                default_action: None,
                session_glob: None,
                session_cli_list: None,
                resume_command: None,
                permission_marker: None,
                approval_channel: None,
                approval_target: None,
            }],
            preferences: HashMap::new(),
            privacy: PrivacyConfig {
                excluded_paths: default_exclusions(),
                excluded_apps: vec![],
            },
        }
    }

    /// Platform config directory for Handover:
    /// * macOS: `~/Library/Application Support/handover`
    /// * Linux: `~/.config/handover` (XDG)
    /// * Windows: `%APPDATA%\handover`
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("handover")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    /// Local HTTP API bearer token file (mode 0600). Separate from the TOML
    /// config so agents/preferences can be shared without leaking the token.
    pub fn api_token_path() -> PathBuf {
        Self::config_dir().join("api_token")
    }

    /// Tiny persisted per-agent session state (mode 0600), next to config:
    /// currently the user's *pinned* session per agent (see the session-aware
    /// handoff design). NOT part of HandoffHistory — survives restarts.
    pub fn session_state_path() -> PathBuf {
        Self::config_dir().join("session_state.json")
    }

    /// Human-readable description of where config lives on this platform.
    pub fn config_path_display() -> String {
        Self::config_path().display().to_string()
    }

    /// Loads the API token, creating a new one if missing.
    pub fn ensure_api_token() -> Result<String, ConfigError> {
        Self::ensure_api_token_at(&Self::api_token_path())
    }

    /// Same as [`ensure_api_token`] but for an explicit path (used by tests).
    pub fn ensure_api_token_at(path: &std::path::Path) -> Result<String, ConfigError> {
        if path.exists() {
            let existing = std::fs::read_to_string(path)?;
            let trimmed = existing.trim().to_string();
            if !trimmed.is_empty() {
                return Ok(trimmed);
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let token = format!("ho_{}", uuid::Uuid::new_v4().simple());
        write_private_string(path, &token)?;
        log::info!("wrote local API token to {path:?}");
        Ok(token)
    }

    /// Reads an existing API token without creating one.
    pub fn load_api_token() -> Option<String> {
        Self::load_api_token_at(&Self::api_token_path())
    }

    pub fn load_api_token_at(path: &std::path::Path) -> Option<String> {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    pub fn load_from(path: &std::path::Path) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&raw)?;
        Ok(config)
    }

    /// Loads the config, creating the default config file on first run.
    ///
    /// Recovery policy:
    /// * **Missing file** → write defaults.
    /// * **Parse error** → rename the bad file to `*.toml.bak`, then write
    ///   defaults. If the rename fails, leave the original file **untouched**
    ///   and return in-memory defaults (never overwrite a broken-but-still-
    ///   recoverable file).
    /// * **I/O / permission errors** → keep the existing file intact, return
    ///   in-memory defaults and log an error.
    ///
    /// Also ensures a local API token file exists.
    pub fn ensure() -> Self {
        let config = Self::ensure_at(&Self::config_path());
        if let Err(e) = Self::ensure_api_token() {
            log::warn!("could not ensure API token: {e}");
        }
        config
    }

    /// Same as the load/recovery half of [`ensure`], for an explicit path
    /// (used by tests; does not touch the API token file).
    pub fn ensure_at(path: &std::path::Path) -> Self {
        match Self::load_from(path) {
            Ok(config) => config,
            Err(ConfigError::Parse(err)) => {
                // Only replace the on-disk file after a successful backup rename.
                if path.exists() {
                    let backup = path.with_extension("toml.bak");
                    match std::fs::rename(path, &backup) {
                        Ok(()) => {
                            log::warn!("backed up unparseable config to {backup:?}: {err}");
                            return write_defaults_to(path);
                        }
                        Err(e) => {
                            log::error!(
                                "could not back up unparseable config {path:?} ({e}); leaving the file untouched and using in-memory defaults"
                            );
                            return Self::default_config();
                        }
                    }
                }
                // Parse error but file gone (race) — safe to materialize defaults.
                write_defaults_to(path)
            }
            Err(ConfigError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                write_defaults_to(path)
            }
            Err(err) => {
                // Permission denied, disk error, etc. — do NOT rename/overwrite.
                log::error!(
                    "could not load config at {path:?}: {err}; using in-memory defaults without modifying the file"
                );
                Self::default_config()
            }
        }
    }

    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Self::config_path())
    }

    pub fn save_to(&self, path: &std::path::Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Message(format!("{e}")))?;
        }
        let raw = toml::to_string_pretty(self).map_err(|e| Message(format!("{e}")))?;
        // config.toml can carry agent env vars and commands — owner-only,
        // matching the API token and session state files.
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .map_err(|e| Message(format!("{e}")))?;
            f.write_all(raw.as_bytes())
                .map_err(|e| Message(format!("{e}")))?;
            // An existing file keeps any prior (looser) mode — enforce 0600.
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| Message(format!("{e}")))?;
        }
        #[cfg(not(unix))]
        std::fs::write(path, raw).map_err(|e| Message(format!("{e}")))?;
        Ok(())
    }

    /// Validates every configured agent (additive — plain command agents are
    /// always valid; session-aware fields are checked when present).
    pub fn validate(&self) -> Result<(), String> {
        for agent in &self.agents {
            agent.validate()?;
        }
        Ok(())
    }

    pub fn agent(&self, id: &str) -> Option<&AgentConfig> {
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn preference_for(&self, action_id: &str) -> Option<&String> {
        self.preferences.get(action_id)
    }

    pub fn set_preference(&mut self, action_id: &str, agent_id: &str) {
        self.preferences
            .insert(action_id.to_string(), agent_id.to_string());
    }
}

/// The built-in catalog of session-aware agents (reference adapters).
///
/// Every entry is a full [`AgentConfig`] with the session-aware fields from
/// the Phase 0 facts table (see ARCHITECTURE.md, "Per-agent session facts"). They are NOT
/// written into the user's config.toml — the daemon overlays them on demand
/// so session-awareness works with zero configuration. Any entry can be
/// overridden by configuring an agent with the same id.
///
/// PRIVACY: `session_glob` is a filenames + mtimes scan only; the one
/// exception is Hermes (`session_cli_list` → `hermes sessions list`), a
/// read-only metadata listing. No transcript contents are ever read here.
pub fn builtin_session_agents() -> Vec<AgentConfig> {
    vec![
        AgentConfig {
            id: "claude".into(),
            name: "Claude Code".into(),
            kind: AgentKind::Session,
            command: "claude -p \"{PROMPT}\"".into(),
            description: Some(
                "Anthropic's Claude Code CLI. Resumes into live sessions by id (--resume).".into(),
            ),
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: Some(300),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.claude/projects/*/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some("claude -p \"{PROMPT}\" --resume {SESSION}".into()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        },
        AgentConfig {
            id: "codex".into(),
            name: "Codex".into(),
            kind: AgentKind::Session,
            command: "codex exec --skip-git-repo-check \"{PROMPT}\"".into(),
            description: Some(
                "OpenAI's Codex CLI. Resumes into live sessions by id (exec resume).".into(),
            ),
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: Some(300),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.codex/sessions/*/*/*/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some(
                "codex exec resume {SESSION} --skip-git-repo-check \"{PROMPT}\"".into(),
            ),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        },
        AgentConfig {
            id: "droid".into(),
            name: "Droid".into(),
            kind: AgentKind::Session,
            command: "droid exec \"{PROMPT}\"".into(),
            description: Some(
                "Factory's Droid CLI. Resumes into live sessions by id (exec -s).".into(),
            ),
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: Some(300),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.factory/sessions/*/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some("droid exec -s {SESSION} \"{PROMPT}\"".into()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        },
        AgentConfig {
            id: "omp".into(),
            name: "Oh My Pi (omp)".into(),
            kind: AgentKind::Session,
            command: "omp -p \"{PROMPT}\"".into(),
            description: Some(
                "Oh My Pi terminal agent. Resumes into on-disk sessions (omp -p -r).".into(),
            ),
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: Some(300),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.omp/agent/sessions/*/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some("omp -p \"{PROMPT}\" -r {SESSION}".into()),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        },
        AgentConfig {
            id: "hermes".into(),
            name: "Hermes".into(),
            kind: AgentKind::Session,
            // -z (oneshot) prints ONLY the final response — no banner/spinner.
            command: "hermes -z \"{PROMPT}\"".into(),
            description: Some(
                "Nous Research's Hermes agent. Sessions live in SQLite (no glob); ".to_string()
                    + "discovered via `hermes sessions list` and resumed with --resume.",
            ),
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: Some(300),
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: None,
            session_cli_list: Some(vec![
                "hermes".into(),
                "sessions".into(),
                "list".into(),
                "--source".into(),
                "cli".into(),
                "--limit".into(),
                "20".into(),
            ]),
            // -Q (quiet) suppresses the banner/spinner/tool previews in chat
            // mode — only the final response + session info come out;
            // --reasoning none stops the unboxed reasoning header leaking
            // onto stdout (quiet mode prints it without a closing border).
            resume_command: Some(
                "hermes chat -Q -q \"{PROMPT}\" --reasoning none --resume {SESSION}".into(),
            ),
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        },
    ]
}

pub fn default_exclusions() -> Vec<String> {
    use handover_core::exclusions::default_excluded_paths;
    default_excluded_paths()
}

fn write_defaults_to(path: &std::path::Path) -> Config {
    let config = Config::default_config();
    if let Err(e) = config.save_to(path) {
        log::error!("could not write default config to {path:?}: {e}");
    } else {
        log::info!("wrote default config to {path:?}");
    }
    config
}

fn write_private_string(path: &std::path::Path, contents: &str) -> Result<(), ConfigError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| Message(format!("{e}")))?;
        file.write_all(contents.as_bytes())
            .map_err(|e| Message(format!("{e}")))?;
        file.write_all(b"\n").map_err(|e| Message(format!("{e}")))?;
        drop(file);
        // `mode(0o600)` applies only on creation: tighten pre-existing files
        // (e.g. token written before this hardening) to owner-only.
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, format!("{contents}\n")).map_err(|e| Message(format!("{e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_demo_agent_and_exclusions() {
        let config = Config::default_config();
        assert_eq!(config.agents.len(), 1);
        assert_eq!(config.agents[0].id, "demo-echo");
        assert!(config.privacy.excluded_paths.contains(&".env".to_string()));
        assert!(config
            .privacy
            .excluded_paths
            .contains(&"~/.ssh/*".to_string()));
        assert_eq!(config.general.history_retention_days, 7);
    }

    #[test]
    fn preferences_roundtrip() {
        let mut config = Config::default_config();
        config.set_preference("fix", "hermes");
        assert_eq!(
            config.preference_for("fix").map(String::as_str),
            Some("hermes")
        );
        assert_eq!(config.preference_for("explain"), None);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("handover-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("config.toml");

        let mut config = Config::default_config();
        config.set_preference("fix", "demo-echo");
        config.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(
            loaded.preference_for("fix").map(String::as_str),
            Some("demo-echo")
        );
        assert_eq!(loaded.agents[0].command, config.agents[0].command);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_loads_default() {
        let path = std::path::Path::new("/nonexistent/handover/config.toml");
        let config = Config::load_from(path);
        assert!(config.is_err());
    }

    #[test]
    fn api_token_roundtrip_is_private() {
        let dir = std::env::temp_dir().join(format!("ho-token-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api_token");

        let token = Config::ensure_api_token_at(&path).expect("token");
        assert!(token.starts_with("ho_"));
        assert_eq!(
            Config::load_api_token_at(&path).as_deref(),
            Some(token.as_str())
        );
        // Second call returns the same token.
        assert_eq!(Config::ensure_api_token_at(&path).unwrap(), token);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "api_token must be owner-only");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_at_backs_up_only_on_parse_error() {
        let dir = std::env::temp_dir().join(format!("ho-ensure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        // Valid → loaded as-is.
        Config::default_config().save_to(&path).unwrap();
        let loaded = Config::ensure_at(&path);
        assert_eq!(loaded.agents[0].id, "demo-echo");
        assert!(!path.with_extension("toml.bak").exists());

        // Corrupt TOML → backup + defaults.
        std::fs::write(&path, "this is not {{{ valid toml").unwrap();
        let recovered = Config::ensure_at(&path);
        assert_eq!(recovered.agents[0].id, "demo-echo");
        assert!(
            path.with_extension("toml.bak").exists(),
            "parse failure must rename the bad file"
        );
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(rewritten.contains("demo-echo"));

        // Unreadable path (directory where a file is expected) → do not clobber.
        let dir_as_file = dir.join("not-a-file");
        std::fs::create_dir_all(&dir_as_file).unwrap();
        let before = std::fs::read_dir(&dir_as_file).unwrap().count();
        let _ = Config::ensure_at(&dir_as_file);
        let after = std::fs::read_dir(&dir_as_file).unwrap().count();
        assert_eq!(
            before, after,
            "I/O failure must not rename/replace a non-file path"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- session-aware agents (Phase 2) ------------------------------------

    #[test]
    fn old_format_config_parses_with_session_defaults() {
        // A config written before session-aware fields existed must load with
        // every new field defaulted (additive schema, serde defaults).
        let old = r#"
[general]
shortcut = "CmdOrCtrl+Shift+A"

[[agents]]
id = "hermes"
name = "Hermes"
kind = "command"
command = "hermes -z \"{PROMPT}\""
enabled = true
"#;
        let config: Config = toml::from_str(old).unwrap();
        assert_eq!(config.agents.len(), 1);
        let agent = &config.agents[0];
        assert_eq!(agent.id, "hermes");
        assert_eq!(agent.kind, AgentKind::Command);
        assert!(
            agent.session_glob.is_none(),
            "old configs default session_glob to None"
        );
        assert!(agent.session_cli_list.is_none());
        assert!(agent.resume_command.is_none());
        assert!(agent.permission_marker.is_none());
        assert!(agent.approval_channel.is_none());
    }

    #[test]
    fn session_agent_roundtrips_through_toml() {
        let agent = AgentConfig {
            id: "codex".into(),
            name: "Codex".into(),
            kind: AgentKind::Session,
            command: "codex exec \"{PROMPT}\"".into(),
            description: None,
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.codex/sessions/*/*/*/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some("codex exec resume {SESSION} \"{PROMPT}\"".into()),
            permission_marker: Some("permission_request".into()),
            approval_channel: Some(ApprovalChannel::Tmux),
            approval_target: Some("my-pane".into()),
        };
        // Validation must accept a complete session agent.
        agent.validate().unwrap();

        let raw = toml::to_string_pretty(&agent).unwrap();
        let back: AgentConfig = toml::from_str(&raw).unwrap();
        assert_eq!(back.kind, AgentKind::Session);
        assert_eq!(
            back.session_glob.as_deref(),
            Some("~/.codex/sessions/*/*/*/*.jsonl")
        );
        assert_eq!(
            back.resume_command.as_deref(),
            Some("codex exec resume {SESSION} \"{PROMPT}\"")
        );
        assert_eq!(
            back.permission_marker.as_deref(),
            Some("permission_request")
        );
        assert_eq!(back.approval_channel, Some(ApprovalChannel::Tmux));
    }

    #[test]
    fn builtin_catalog_entries_are_complete_and_valid() {
        let catalog = builtin_session_agents();
        let ids: Vec<&str> = catalog.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["claude", "codex", "droid", "omp", "hermes"]);

        for agent in &catalog {
            agent.validate().expect("catalog entries must validate");
            assert!(
                agent.resume_command.is_some(),
                "{}: needs resume_command",
                agent.id
            );
            let resume = agent.resume_command.as_deref().unwrap();
            assert!(
                resume.contains("{SESSION}"),
                "{}: resume_command needs {{SESSION}}",
                agent.id
            );
            // Every entry has exactly one discovery mechanism.
            match (&agent.session_glob, &agent.session_cli_list) {
                (Some(g), None) => assert!(!g.is_empty()),
                (None, Some(c)) => assert!(!c.is_empty()),
                other => panic!(
                    "{}: needs exactly one of session_glob/session_cli_list: {other:?}",
                    agent.id
                ),
            }
            // Permission fields default off.
            assert!(agent.permission_marker.is_none());
            assert!(agent.approval_channel.is_none());
        }

        // Hermes is the cli-list entry (SQLite store, no glob).
        let hermes = catalog.iter().find(|a| a.id == "hermes").unwrap();
        assert!(hermes.session_glob.is_none());
        assert_eq!(hermes.session_cli_list.as_ref().unwrap()[0], "hermes");
        assert_eq!(hermes.session_cli_list.as_ref().unwrap()[1], "sessions");
    }

    #[test]
    fn resume_command_without_session_placeholder_is_rejected() {
        let mut agent = AgentConfig {
            id: "broken".into(),
            name: "Broken".into(),
            kind: AgentKind::Session,
            command: "x".into(),
            description: None,
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.x/*.jsonl".into()),
            session_cli_list: None,
            resume_command: Some("agent --prompt \"{PROMPT}\"".into()), // missing {SESSION}
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        };
        let err = agent.validate().unwrap_err();
        assert!(
            err.contains("{SESSION}"),
            "validation must mention {{SESSION}}: {err}"
        );

        // Also rejected at the Config level.
        let mut config = Config::default_config();
        config.agents.push(agent.clone());
        let err = config.validate().unwrap_err();
        assert!(err.contains("broken"));

        // Fixing the resume_command makes it valid.
        agent.resume_command = Some("agent --resume {SESSION} --prompt \"{PROMPT}\"".into());
        agent.validate().unwrap();
    }

    #[test]
    fn session_kind_without_discovery_is_rejected() {
        let agent = AgentConfig {
            id: "ghost".into(),
            name: "Ghost".into(),
            kind: AgentKind::Session,
            command: "x".into(),
            description: None,
            working_dir: None,
            env: HashMap::new(),
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
        };
        assert!(agent.validate().is_err());
    }

    #[test]
    fn glob_and_cli_list_are_mutually_exclusive() {
        let agent = AgentConfig {
            id: "both".into(),
            name: "Both".into(),
            kind: AgentKind::Session,
            command: "x".into(),
            description: None,
            working_dir: None,
            env: HashMap::new(),
            timeout_secs: None,
            enabled: true,
            demo: false,
            default_action: None,
            session_glob: Some("~/.x/*.jsonl".into()),
            session_cli_list: Some(vec!["ls".into()]),
            resume_command: None,
            permission_marker: None,
            approval_channel: None,
            approval_target: None,
        };
        assert!(agent.validate().is_err());
    }

    #[test]
    fn ensure_at_leaves_file_if_backup_rename_fails() {
        let dir = std::env::temp_dir().join(format!("ho-ensure-bak-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let bad = "this is not {{{ valid toml either";
        std::fs::write(&path, bad).unwrap();

        // Occupy the backup name with a directory so rename(config.toml → *.bak) fails.
        let bak = path.with_extension("toml.bak");
        std::fs::create_dir_all(&bak).unwrap();

        let recovered = Config::ensure_at(&path);
        assert_eq!(recovered.agents[0].id, "demo-echo"); // in-memory defaults

        // Original malformed file must still be on disk (not overwritten).
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            on_disk, bad,
            "failed backup must not clobber the broken config"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
