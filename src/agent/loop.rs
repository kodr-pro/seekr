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
    TokenUsage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
    /// Current iteration count updated
    IterationUpdate(u32),
    /// The agent finished its turn (no more tool calls)
    TurnComplete,
    /// Agent loop hit max iterations — waiting for Continue or AnswerNow
    MaxIterationsReached,
    /// An error occurred
    Error(String),
    /// Request tool approval from the user
    ToolApprovalRequest { call_index: usize, name: String, arguments: String },
    /// Request shell stdin input (e.g. sudo password, [y/n] prompt)
    ShellInputNeeded { context: String, input_tx: tokio::sync::mpsc::UnboundedSender<String> },
    /// A new task was created
    TaskCreated(crate::tools::task::Task),
    /// An existing task was updated
    TaskUpdated(crate::tools::task::Task),
}

/// Events sent from the UI to the agent loop
#[derive(Debug, Clone)]
pub enum AgentCommand {
    /// User sent a new message
    UserMessage(String),
    /// User approved a tool call
    ToolApproved { call_index: usize },
    /// User denied a tool call
    ToolDenied { call_index: usize },
    /// User chose "always approve" for the session
    ToolAlwaysApprove,
    /// User provided shell stdin input
    ShellInputResponse(String),
    /// User chose to continue after max iterations
    Continue,
    /// User chose to answer now (stop iterating) after max iterations
    AnswerNow,
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
        session.task_manager = session.task_manager.with_sender(event_tx.clone());
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

    pub fn resume(
        config: AppConfig,
        session_id: &str,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    ) -> Result<Self> {
        let client = DeepSeekClient::new(&config);
        let mut session = Session::load(session_id)?;
        session.task_manager = session.task_manager.with_sender(event_tx.clone());
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
        loop {
            tokio::select! {
                Some(command) = self.command_rx.recv() => {
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
                            self.session.save().ok();
                        }
                        AgentCommand::ToolAlwaysApprove => {
                            self.auto_approve = true;
                        }
                        AgentCommand::Shutdown => break,
                        _ => {}
                    }
                }
            }
        }
    }

    /// Execute one full agent turn: call the API, handle tool calls, repeat
    async fn run_agent_turn(&mut self) {
        'turn: loop {
            // ── INTERRUPT CHECK ──────────────────────────────────────────────
            // Before every API call, drain any commands that arrived while we
            // were busy executing tools. This is what makes the agent responsive:
            // the user's new message is injected directly into the conversation
            // history so the LLM sees it on the next call and can respond.
            match self.drain_pending_commands().await {
                DrainResult::Shutdown => return,
                DrainResult::UserMessageInjected => {
                    // User interrupted. Skip the iteration-limit check and go
                    // straight to the API call so the LLM can respond now.
                    self.event_tx.send(AgentEvent::IterationUpdate(self.iteration)).ok();
                    self.prune_messages();
                    // Jump to API call by falling through — the code below handles it.
                }
                DrainResult::Nothing => {
                    // Normal path: check iteration limit, then call API.
                }
            }

            // Check iteration limit
            if self.iteration >= self.config.agent.max_iterations {
                self.event_tx.send(AgentEvent::MaxIterationsReached).ok();
                // Suspend: wait for Continue or AnswerNow from the UI
                match self.wait_for_continue_or_answer().await {
                    ContinueAction::Continue => {
                        // Reset iteration count and keep looping
                        self.iteration = 0;
                        self.event_tx.send(AgentEvent::IterationUpdate(self.iteration)).ok();
                    }
                    ContinueAction::AnswerNow => {
                        // Make one final API call to give a clean summary answer
                        self.session.messages.push(ChatMessage::user(
                            "Please stop what you're doing and give me a concise answer or summary of what you've accomplished so far. Do not use any tools."
                        ));
                        self.event_tx.send(AgentEvent::IterationUpdate(0)).ok();
                        self.do_final_answer().await;
                        return;
                    }
                    ContinueAction::Shutdown => return,
                }
            }

            self.iteration += 1;
            self.event_tx.send(AgentEvent::IterationUpdate(self.iteration)).ok();

            // Prune context if needed before calling API
            self.prune_messages();


            // Call the API with streaming
            let tool_defs = tools::all_tool_definitions(Some(&self.config.agent.working_directory));
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
                    self.event_tx.send(AgentEvent::Error(format!("API error: {e}"))).ok();
                    break;
                }
            };

            // Collect the full response from the stream
            let mut content_buf = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            loop {
                tokio::select! {
                    Some(event) = stream_rx.recv() => {
                        match event {
                            StreamEvent::ContentDelta(text) => {
                                content_buf.push_str(&text);
                                self.event_tx.send(AgentEvent::ContentDelta(text)).ok();
                            }
                            StreamEvent::ReasoningDelta(text) => {
                                self.event_tx.send(AgentEvent::ReasoningDelta(text)).ok();
                            }
                            StreamEvent::ToolCallComplete(tc) => {
                                tool_calls.push(tc);
                            }
                            StreamEvent::Usage { prompt_tokens, completion_tokens, total_tokens } => {
                                self.event_tx.send(AgentEvent::TokenUsage { prompt_tokens, completion_tokens, total_tokens }).ok();
                            }
                            StreamEvent::Done => break,
                            StreamEvent::Error(e) => {
                                self.event_tx.send(AgentEvent::Error(format!("Stream error: {e}"))).ok();
                                break;
                            }
                        }
                    }
                    Some(AgentCommand::UserMessage(msg)) = self.command_rx.recv() => {
                        // USER INTERRUPT mid-stream
                        self.session.messages.push(ChatMessage::user(&msg));
                        self.event_tx.send(AgentEvent::Activity(crate::tools::ActivityEntry {
                            tool_name: "chat".to_string(),
                            summary: "Interrupted by user message".to_string(),
                            status: crate::tools::ActivityStatus::Success,
                            timestamp: chrono::Utc::now(),
                        })).ok();
                        
                        // Finalize what we have so far
                        if !content_buf.is_empty() {
                             self.session.messages.push(ChatMessage::assistant(&content_buf));
                        }
                        
                        // Restart the turn (reset iteration and jump to top of 'turn loop)
                        self.iteration = 0;
                        self.session.save().ok();
                        continue 'turn;
                    }
                    else => break,
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
                    self.event_tx.send(AgentEvent::ToolCallStart {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    }).ok();

                    // Check approval if needed
                    if !self.auto_approve {
                        // Use the call's original ID or index for reference
                        self.event_tx.send(AgentEvent::ToolApprovalRequest {
                            call_index: tool_calls.iter().position(|t| t.id == tc.id).unwrap_or(0),
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        }).ok();

                        // Wait for approval
                        let approved = self.wait_for_approval().await;
                        if !approved {
                            self.session.messages.push(ChatMessage::tool_result(
                                &tc.id,
                                "Tool execution denied by user.",
                            ));
                            self.event_tx.send(AgentEvent::ToolCallResult {
                                name: tc.function.name.clone(),
                                result: "Denied by user".to_string(),
                            }).ok();
                            continue;
                        }
                    }

                    // Special case: task tools must be executed sequentially because they mutate state
                    if tc.function.name.contains("task") {
                        let (result, activity) = tools::execute_tool(
                            &tc.function.name,
                            &tc.function.arguments,
                            &mut self.session.task_manager,
                            Some(&self.config.agent.working_directory),
                        )
                        .await;

                        self.event_tx.send(AgentEvent::Activity(activity)).ok();
                        self.event_tx.send(AgentEvent::ToolCallResult {
                            name: tc.function.name.clone(),
                            result: result.clone(),
                        }).ok();

                        self.session.messages.push(ChatMessage::tool_result(&tc.id, &result));
                    } else {
                        // Other tools can be prepared for parallel execution
                        // We need to clone what we need since we'll be moving into a future
                        let name = tc.function.name.clone();
                        let arguments = tc.function.arguments.clone();
                        let id = tc.id.clone();
                        let event_tx_clone = self.event_tx.clone();
                        let wd = self.config.agent.working_directory.clone();
                        
                        tool_futures.push(async move {
                            let mut dummy_tm = TaskManager::new().with_sender(event_tx_clone); 
                            let (result, activity) = tools::execute_tool(&name, &arguments, &mut dummy_tm, Some(&wd)).await;
                            (id, name, result, activity)
                        });
                    }
                }

                // Execute independent tools in parallel
                if !tool_futures.is_empty() {
                    let results = futures::future::join_all(tool_futures).await;
                    for (id, name, result, activity) in results {
                        self.event_tx.send(AgentEvent::Activity(activity)).ok();
                        self.event_tx.send(AgentEvent::ToolCallResult {
                            name,
                            result: result.clone(),
                        }).ok();
                        self.session.messages.push(ChatMessage::tool_result(&id, &result));
                    }
                }

                // Continue the loop - call the API again with tool results
                self.session.save().ok();
                continue;
            }

            // No tool calls - this is a regular text response, turn is complete
            if !content_buf.is_empty() {
                self.session.messages.push(ChatMessage::assistant(&content_buf));
            }
            self.session.save().ok();
            self.event_tx.send(AgentEvent::TurnComplete).ok();
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

    /// Wait for the user to decide what to do after max iterations are reached.
    async fn wait_for_continue_or_answer(&mut self) -> ContinueAction {
        while let Some(cmd) = self.command_rx.recv().await {
            match cmd {
                AgentCommand::Continue => return ContinueAction::Continue,
                AgentCommand::AnswerNow => return ContinueAction::AnswerNow,
                AgentCommand::Shutdown => return ContinueAction::Shutdown,
                // A new user message acts as an override — inject and continue
                AgentCommand::UserMessage(msg) => {
                    // The user typed something new — treat as "answer now by responding to the user"
                    self.session.messages.push(ChatMessage::user(&msg));
                    return ContinueAction::AnswerNow;
                }
                _ => {}
            }
        }
        ContinueAction::Shutdown
    }

    /// Do one non-tool API call to produce a final answer/summary
    async fn do_final_answer(&mut self) {
        // Prune first
        self.prune_messages();

        let stream_result = self
            .client
            .chat_completion_stream(
                self.session.messages.clone(),
                &self.config.api.model,
                None, // no tools — forces a text response
            )
            .await;

        let mut stream_rx = match stream_result {
            Ok(rx) => rx,
            Err(e) => {
                self.event_tx.send(AgentEvent::Error(format!("API error: {e}"))).ok();
                self.event_tx.send(AgentEvent::TurnComplete).ok();
                return;
            }
        };

        let mut content_buf = String::new();
        while let Some(event) = stream_rx.recv().await {
            match event {
                crate::api::stream::StreamEvent::ContentDelta(text) => {
                    content_buf.push_str(&text);
                    self.event_tx.send(AgentEvent::ContentDelta(text)).ok();
                }
                crate::api::stream::StreamEvent::Usage { prompt_tokens, completion_tokens, total_tokens } => {
                    self.event_tx.send(AgentEvent::TokenUsage { prompt_tokens, completion_tokens, total_tokens }).ok();
                }
                crate::api::stream::StreamEvent::Done => break,
                crate::api::stream::StreamEvent::Error(e) => {
                    self.event_tx.send(AgentEvent::Error(format!("Stream error: {e}"))).ok();
                    break;
                }
                _ => {}
            }
        }

        if !content_buf.is_empty() {
            self.session.messages.push(ChatMessage::assistant(&content_buf));
        }
        self.session.save().ok();
        self.event_tx.send(AgentEvent::TurnComplete).ok();
    }

    /// Prune messages to fit within context limits.
    /// IMPORTANT: never orphan a tool_result message — a tool_result MUST be
    /// preceded by an assistant message that has tool_calls. If we slice mid-pair
    /// the API returns a 400 error.
    fn prune_messages(&mut self) {
        const MAX_MESSAGES: usize = 60;

        if self.session.messages.len() <= MAX_MESSAGES {
            return;
        }

        let system_prompt = self.session.messages[0].clone();
        let total = self.session.messages.len();
        // Start from the naive window
        let mut start = total - MAX_MESSAGES;

        // Walk start forward until the first message at `start` is NOT a tool-result.
        // Tool-result messages have role == "tool". If we start on one, we'd
        // orphan it from its preceding assistant+tool_calls message.
        while start < total {
            let role = self.session.messages[start].role.as_str();
            if role != "tool" {
                break;
            }
            start += 1;
        }

        let mut recent = self.session.messages[start..].to_vec();
        let mut new_messages = vec![system_prompt];
        new_messages.append(&mut recent);
        self.session.messages = new_messages;
    }

    pub fn task_manager(&self) -> &TaskManager {
        &self.session.task_manager
    }

    /// Non-blocking drain of the command channel.
    /// Injects any pending user messages into conversation history so
    /// the next API call can respond to them.
    /// Returns whether anything meaningful was found.
    async fn drain_pending_commands(&mut self) -> DrainResult {
        let mut found_user_message = false;

        loop {
            match self.command_rx.try_recv() {
                Ok(AgentCommand::UserMessage(msg)) => {
                    // Inject directly into history — the UI already showed it
                    self.session.messages.push(ChatMessage::user(&msg));
                    found_user_message = true;
                    // Keep draining in case more messages arrived
                }
                Ok(AgentCommand::Shutdown) => return DrainResult::Shutdown,
                Ok(AgentCommand::ToolAlwaysApprove) => {
                    self.auto_approve = true;
                }
                Ok(_) => {} // Ignore other commands (Continue/AnswerNow don't apply here)
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    return DrainResult::Shutdown;
                }
            }
        }

        if found_user_message {
            DrainResult::UserMessageInjected
        } else {
            DrainResult::Nothing
        }
    }
}

/// Result of waiting for continue-or-answer decision
enum ContinueAction {
    Continue,
    AnswerNow,
    Shutdown,
}

/// Result of draining the command channel between tool calls
enum DrainResult {
    /// A user message was found and injected into conversation history
    UserMessageInjected,
    /// A shutdown command was received
    Shutdown,
    /// Nothing noteworthy
    Nothing,
}
