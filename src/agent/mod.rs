// agent/mod.rs - Agent module root
//
// Re-exports the agent loop and system prompt components.

#[allow(clippy::module_inception)]
pub mod r#loop;
pub mod system_prompt;

pub use r#loop::{AgentCommand, AgentEvent, AgentLoop};
