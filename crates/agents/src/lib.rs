//! Agent integrations. Every integration implements the core `Agent` trait
//! and stays isolated behind it.

pub mod generic_command;
pub mod registry;
pub mod session_agent;

pub use generic_command::GenericCommandAgent;
pub use registry::AgentRegistry;
pub use session_agent::SessionAgent;
