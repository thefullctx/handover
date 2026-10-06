pub mod adapter;
pub mod config;

pub use adapter::{compat_for, compat_for_config, AdapterCompat, ADAPTER_COMPAT};
pub use config::{
    atomic_write_private, builtin_session_agents, clamp_ui_opacity, AgentConfig, AgentKind,
    Appearance, ApprovalChannel, Config, ConfigError, GeneralConfig, PrivacyConfig,
};
