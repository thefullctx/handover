use std::sync::Arc;

use handover_config::{AgentConfig, AgentKind};
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
