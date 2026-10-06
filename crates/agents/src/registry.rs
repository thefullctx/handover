use std::sync::Arc;

use handover_config::{compat_for_config, AgentConfig, AgentKind};
use handover_core::agent::{
    Agent, AgentError, AgentMetaStatus, AgentRequest, AgentStatus, SendReceipt,
};

use crate::generic_command::GenericCommandAgent;
use crate::session_agent::SessionAgent;

/// Holds every configured (and enabled) agent behind the `Agent` trait.
///
/// Agents are stored as `Arc<dyn Agent>` so a handoff can be resolved
/// (looked up) under the daemon lock and then *executed* with the lock
/// released — a long-running agent must never block the palette or the API.
pub struct AgentRegistry {
    agents: Vec<Arc<dyn Agent>>,
}

impl AgentRegistry {
    pub fn from_configs(configs: &[AgentConfig]) -> Self {
        let mut agents: Vec<Arc<dyn Agent>> = Vec::new();
        for config in configs {
            if !config.enabled {
                continue;
            }
            // An invalid config (e.g. a hand-edited resume_command missing
            // {SESSION}) must not crash the daemon: skip the agent and warn.
            // It stays in config.agents (visible in Settings to fix) but is
            // absent from the registry, so it cannot be targeted until fixed.
            if let Err(e) = config.validate() {
                log::warn!("skipping invalid agent `{}`: {e}", config.id);
                continue;
            }
            match config.kind {
                AgentKind::Command => {
                    agents.push(Arc::new(GenericCommandAgent::new(config.clone())))
                }
                AgentKind::Session => {
                    // Session-aware: resumes into a live session when the
                    // request targets one; otherwise identical to a fresh
                    // generic-command send.
                    agents.push(Arc::new(SessionAgent::new(config.clone())))
                }
                AgentKind::OpenAiCompatible => agents.push(Arc::new(UnsupportedAgent::new(
                    config.clone(),
                    "OpenAI-compatible endpoints arrive in a later milestone.",
                ))),
            }
        }
        Self { agents }
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Agent>> {
        self.agents.iter().find(|a| a.meta().id == id).cloned()
    }

    /// The default agent: the first enabled one.
    pub fn default_id(&self) -> Option<String> {
        self.agents.first().map(|a| a.meta().id)
    }

    pub fn statuses(&self) -> Vec<AgentMetaStatus> {
        self.agents
            .iter()
            .map(|a| AgentMetaStatus {
                meta: a.meta(),
                status: a.detect().unwrap_or(AgentStatus {
                    available: false,
                    detail: "agent probe failed".to_string(),
                    running: false,
                    provider_down: None,
                }),
            })
            .collect()
    }

    pub fn send(&self, id: &str, request: &AgentRequest) -> Result<SendReceipt, AgentError> {
        match self.get(id) {
            Some(agent) => agent.send(request, None),
            None => Err(AgentError::Message(format!("Unknown agent `{id}`"))),
        }
    }
}

/// A placeholder for agent kinds not implemented yet. It reports honestly
/// that it is unavailable instead of silently disappearing.
struct UnsupportedAgent {
    config: AgentConfig,
    reason: String,
}

impl UnsupportedAgent {
    fn new(config: AgentConfig, reason: &str) -> Self {
        Self {
            config,
            reason: reason.to_string(),
        }
    }
}

impl Agent for UnsupportedAgent {
    fn meta(&self) -> handover_core::agent::AgentMeta {
        handover_core::agent::AgentMeta {
            id: self.config.id.clone(),
            name: self.config.name.clone(),
            description: self.config.description.clone().unwrap_or_default(),
            kind: "openai_compatible".to_string(),
            config_summary: None,
            compat: compat_for_config(&self.config).map(|c| c.summary()),
            demo: self.config.demo,
        }
    }

    fn detect(&self) -> Result<AgentStatus, AgentError> {
        Ok(AgentStatus {
            available: false,
            detail: self.reason.clone(),
            running: false,
            provider_down: None,
        })
    }

    fn send(
        &self,
        _request: &AgentRequest,
        _stream: Option<handover_core::agent::OutputSink>,
    ) -> Result<SendReceipt, AgentError> {
        Err(AgentError::Unavailable {
            agent: self.config.name.clone(),
            detail: self.reason.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.into(),
            name: format!("Agent {id}"),
            kind: AgentKind::Command,
            command: "true".into(),
            description: None,
            working_dir: None,
            env: std::collections::HashMap::new(),
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
        }
    }

    #[test]
    fn invalid_config_is_skipped_with_warning() {
        // A hand-edited config must not crash the daemon — the invalid agent
        // is skipped (with a warn log) while valid ones still load.
        let mut invalid = config("broken");
        invalid.resume_command = Some("agent --prompt \"{PROMPT}\"".into()); // missing {SESSION}
        let valid = config("good");
        let registry = AgentRegistry::from_configs(&[invalid, valid]);
        assert!(registry.get("broken").is_none());
        assert!(registry.get("good").is_some());
    }

    #[test]
    fn meta_carries_the_adapter_compat_declaration() {
        // A bundled agent surfaces which version its adapter was verified
        // against, so a drift report can be weighed against what the user has.
        let mut bundled = config("codex");
        bundled.kind = AgentKind::Session;
        bundled.command = "codex exec --skip-git-repo-check \"{PROMPT}\"".into();
        bundled.session_glob = Some("~/.codex/sessions/*/*/*/*.jsonl".into());
        bundled.resume_command = Some("codex exec resume {SESSION}".into());
        let registry = AgentRegistry::from_configs(&[bundled]);

        let meta = registry.get("codex").expect("codex registered").meta();
        let compat = meta.compat.expect("bundled agent declares compat");
        assert!(
            compat.contains("adapter:") && compat.contains("verified"),
            "compat must name the verified version and date: {compat}"
        );
    }

    #[test]
    fn meta_has_no_compat_for_a_users_own_command_agent() {
        // A hand-written command agent carries no bundled assumptions, so it
        // must not be given a declaration it never earned — even when it
        // reuses a bundled agent's id but runs a different program.
        let mut lookalike = config("claude");
        lookalike.command = "./my-script.sh \"{PROMPT}\"".into();
        let registry = AgentRegistry::from_configs(&[config("my-own-agent"), lookalike]);
        for id in ["my-own-agent", "claude"] {
            let meta = registry.get(id).expect("registered").meta();
            assert!(
                meta.compat.is_none(),
                "{id}: unexpected compat: {:?}",
                meta.compat
            );
        }
    }

    #[test]
    fn disabled_config_is_skipped() {
        let mut disabled = config("off");
        disabled.enabled = false;
        let registry = AgentRegistry::from_configs(&[disabled]);
        assert!(registry.is_empty());
    }
}
