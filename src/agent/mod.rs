// agent/mod.rs - Agent module root
//
// Re-exports the agent loop and system prompt components.

#[allow(clippy::module_inception)]
#[path = "loop.rs"]
pub mod loop_mod;
pub mod system_prompt;

pub use loop_mod::{AgentCommand, AgentEvent, AgentLoop};
