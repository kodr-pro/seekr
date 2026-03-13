// agent/loop.rs - Agent reasoning loop (plan → act → observe)
//
// Manages the conversation with the DeepSeek API, including streaming
// responses, tool call handling, and the iterative agent loop.

use tokio::sync::mpsc;

use crate::api::client::DeepSeekClient;
use crate::api::stream::StreamEvent;
use crate::api::types::*;
use crate::config::AppConfig;
use crate::tools;
use crate::tools::task::TaskManager;

use crate::session::Session;
use anyhow::Result;
use super::system_prompt::build_system_prompt;

/// Events sent from the agent loop to the UI
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A chunk of assistant text content arrived
    ContentDelta(String),
    /// A chunk of reasoning content arrived
    ReasoningDelta(String),
    /// The assistant wants to call a tool (displayed before execution)
    ToolCallStart { name: String, arguments: String },
    /// A tool finished executing with this result
    ToolCallResult { name: String, result: String },
    /// Activity log entry
    Activity(tools::ActivityEntry),
    /// Token usage update
    #[allow(dead_code)]
    TokenUsage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
    /// The agent finished its turn (no more tool calls)
    TurnComplete,
    /// Agent loop hit max iterations
    MaxIterationsReached,
    /// An error occurred
    Error(String),
    /// Request tool approval from the user
    #[allow(dead_code)]
    ToolApprovalRequest { call_index: usize, name: String, arguments: String },
}

/// Events sent from the UI to the agent loop
#[derive(Debug, Clone)]
pub enum AgentCommand {
    /// User sent a new message
    UserMessage(String),
    /// User approved a tool call
    #[allow(dead_code)]
    ToolApproved { call_index: usize },
    /// User denied a tool call
    #[allow(dead_code)]
    ToolDenied { call_index: usize },
    /// User chose "always approve" for the session
    ToolAlwaysApprove,
    /// Shutdown the agent
    Shutdown,
}

/// The agent loop task, runs in a background tokio task
pub struct AgentLoop {
    client: DeepSeekClient,
    config: AppConfig,
    session: Session,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    auto_approve: bool,
    iteration: u32,
}

impl AgentLoop {
    pub fn new(
        config: AppConfig,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    ) -> Self {
        let client = DeepSeekClient::new(&config);
        let system_prompt = build_system_prompt(&config.agent.working_directory);
        let auto_approve = config.agent.auto_approve_tools;

        // Start a new session with a random ID
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut session = Session::new(session_id, "New Chat".to_string());
        session.messages.push(ChatMessage::system(&system_prompt));

        Self {
            client,
            config,
            session,
            event_tx,
            command_rx,
            auto_approve,
            iteration: 0,
        }
    }

    /// Resume a session from disk
    #[allow(dead_code)]
    pub fn resume(
        config: AppConfig,
        session_id: &str,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    ) -> Result<Self> {
        let client = DeepSeekClient::new(&config);
        let session = Session::load(session_id)?;
        let auto_approve = config.agent.auto_approve_tools;

        Ok(Self {
            client,
            config,
            session,
            event_tx,
            command_rx,
            auto_approve,
            iteration: 0,
        })
    }

    /// Run the agent loop, processing commands from the UI
    pub async fn run(mut self) {
        while let Some(command) = self.command_rx.recv().await {
            match command {
                AgentCommand::UserMessage(msg) => {
                    // Update session title on first message if it's still default
                    if self.session.title == "New Chat" {
                        let title = if msg.len() > 30 {
                            format!("{}...", &msg[..27])
                        } else {
                            msg.clone()
                        };
                        self.session.title = title;
                    }
                    
                    self.session.messages.push(ChatMessage::user(&msg));
                    self.iteration = 0;
                    self.run_agent_turn().await;
                    let _ = self.session.save();
                }
                AgentCommand::ToolAlwaysApprove => {
                    self.auto_approve = true;
                }
                AgentCommand::Shutdown => {
                    break;
                }
                // Approval/denial handled within run_agent_turn
                _ => {}
            }
        }
    }

    /// Execute one full agent turn: call the API, handle tool calls, repeat
    async fn run_agent_turn(&mut self) {
        loop {
            // Check iteration limit
            if self.iteration >= self.config.agent.max_iterations {
                let _ = self.event_tx.send(AgentEvent::MaxIterationsReached);
                break;
            }
            self.iteration += 1;

            // Prune context if needed before calling API
            self.prune_messages();

            // Call the API with streaming
            let tool_defs = tools::all_tool_definitions();
            let stream_result = self
                .client
                .chat_completion_stream(
                    self.session.messages.clone(),
                    &self.config.api.model,
                    Some(tool_defs),
                )
                .await;

            let mut stream_rx = match stream_result {
                Ok(rx) => rx,
                Err(e) => {
                    let _ = self
                        .event_tx
                        .send(AgentEvent::Error(format!("API error: {e}")));
                    break;
                }
            };

            // Collect the full response from the stream
            let mut content_buf = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            while let Some(event) = stream_rx.recv().await {
                match event {
                    StreamEvent::ContentDelta(text) => {
                        content_buf.push_str(&text);
                        let _ = self.event_tx.send(AgentEvent::ContentDelta(text));
                    }
                    StreamEvent::ReasoningDelta(text) => {
                        let _ = self.event_tx.send(AgentEvent::ReasoningDelta(text));
                    }
                    StreamEvent::ToolCallComplete(tc) => {
                        tool_calls.push(tc);
                    }
                    StreamEvent::Usage {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                    } => {
                        let _ = self.event_tx.send(AgentEvent::TokenUsage {
                            prompt_tokens,
                            completion_tokens,
                            total_tokens,
                        });
                    }
                    StreamEvent::Done => break,
                    StreamEvent::Error(e) => {
                        let _ = self
                            .event_tx
                            .send(AgentEvent::Error(format!("Stream error: {e}")));
                        break;
                    }
                }
            }

            // If there are tool calls, handle them
            if !tool_calls.is_empty() {
                // Add the assistant message with tool calls to history
                let content = if content_buf.is_empty() {
                    None
                } else {
                    Some(content_buf.clone())
                };
                self.session.messages.push(ChatMessage::assistant_with_tool_calls(
                    content,
                    tool_calls.clone(),
                ));

                // Execute each tool call
                let mut tool_futures = Vec::new();
                
                for tc in &tool_calls {
                    let _ = self.event_tx.send(AgentEvent::ToolCallStart {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    });

                    // Check approval if needed
                    if !self.auto_approve {
                        let _ = self.event_tx.send(AgentEvent::ToolApprovalRequest {
                            call_index: 0, // This logic needs improvement for multiple tools
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        });

                        // Wait for approval
                        let approved = self.wait_for_approval().await;
                        if !approved {
                            self.session.messages.push(ChatMessage::tool_result(
                                &tc.id,
                                "Tool execution denied by user.",
                            ));
                            let _ = self.event_tx.send(AgentEvent::ToolCallResult {
                                name: tc.function.name.clone(),
                                result: "Denied by user".to_string(),
                            });
                            continue;
                        }
                    }

                    // Special case: task tools must be executed sequentially because they mutate state
                    if tc.function.name.contains("task") {
                        let (result, activity) = tools::execute_tool(
                            &tc.function.name,
                            &tc.function.arguments,
                            &mut self.session.task_manager,
                        )
                        .await;

                        let _ = self.event_tx.send(AgentEvent::Activity(activity));
                        let _ = self.event_tx.send(AgentEvent::ToolCallResult {
                            name: tc.function.name.clone(),
                            result: result.clone(),
                        });

                        self.session.messages.push(ChatMessage::tool_result(&tc.id, &result));
                    } else {
                        // Other tools can be prepared for parallel execution
                        // We need to clone what we need since we'll be moving into a future
                        let name = tc.function.name.clone();
                        let arguments = tc.function.arguments.clone();
                        let id = tc.id.clone();
                        
                        tool_futures.push(async move {
                            // Note: We bypass TaskManager here because non-task tools don't use it
                            // This is a bit of a hack until we have a better registry
                            let mut dummy_tm = TaskManager::new(); 
                            let (result, activity) = tools::execute_tool(&name, &arguments, &mut dummy_tm).await;
                            (id, name, result, activity)
                        });
                    }
                }

                // Execute independent tools in parallel
                if !tool_futures.is_empty() {
                    let results = futures::future::join_all(tool_futures).await;
                    for (id, name, result, activity) in results {
                        let _ = self.event_tx.send(AgentEvent::Activity(activity));
                        let _ = self.event_tx.send(AgentEvent::ToolCallResult {
                            name,
                            result: result.clone(),
                        });
                        self.session.messages.push(ChatMessage::tool_result(&id, &result));
                    }
                }

                // Continue the loop - call the API again with tool results
                continue;
            }

            // No tool calls - this is a regular text response, turn is complete
            if !content_buf.is_empty() {
                self.session.messages.push(ChatMessage::assistant(&content_buf));
            }
            let _ = self.session.save();
            let _ = self.event_tx.send(AgentEvent::TurnComplete);
            break;
        }
    }

    /// Wait for user approval of a tool call
    async fn wait_for_approval(&mut self) -> bool {
        while let Some(cmd) = self.command_rx.recv().await {
            match cmd {
                AgentCommand::ToolApproved { .. } => return true,
                AgentCommand::ToolDenied { .. } => return false,
                AgentCommand::ToolAlwaysApprove => {
                    self.auto_approve = true;
                    return true;
                }
                AgentCommand::Shutdown => return false,
                _ => {}
            }
        }
        false
    }

    /// Prune messages to fit within context limits
    fn prune_messages(&mut self) {
        // Simple sliding window: keep system prompt and last 20 messages
        // This is a naive implementation; token-based pruning would be better.
        const MAX_MESSAGES: usize = 20;

        if self.session.messages.len() > MAX_MESSAGES {
            let system_prompt = self.session.messages[0].clone();
            let mut recent = self.session.messages[self.session.messages.len() - MAX_MESSAGES..].to_vec();
            
            // Ensure we keep the system prompt at the start
            let mut new_messages = vec![system_prompt];
            new_messages.append(&mut recent);
            self.session.messages = new_messages;
        }
    }

    /// Get task manager reference (for UI rendering)
    #[allow(dead_code)]
    pub fn task_manager(&self) -> &TaskManager {
        &self.session.task_manager
    }
}
