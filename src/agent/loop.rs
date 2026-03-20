use tokio::sync::mpsc;
use crate::api::client::ApiClient;
use crate::api::stream::StreamEvent;
use crate::api::types::*;
use crate::config::AppConfig;
use crate::tools;
use crate::tools::task::TaskManager;
use crate::session::Session;
use anyhow::Result;
use super::system_prompt::build_system_prompt;
use crate::tools::SkillRegistry;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum AgentEvent {
    ContentDelta(String),
    ReasoningDelta(String),
    ToolCallStart { name: String, arguments: String },
    ToolCallResult { name: String, result: String },
    Activity(tools::ActivityEntry),
    TokenUsage { prompt_tokens: u32, completion_tokens: u32, total_tokens: u32 },
    IterationUpdate(u32),
    TurnComplete,
    MaxIterationsReached,
    Error(String),
    ToolApprovalRequest { call_index: usize, name: String, arguments: String },
    ShellInputNeeded { context: String, input_tx: tokio::sync::mpsc::UnboundedSender<String> },
    TaskCreated(crate::tools::task::Task),
    TaskUpdated(crate::tools::task::Task),
    ContextPruned { count: usize },
    ContextSummaryReady { id: String, summary: String },
}

#[derive(Debug, Clone)]
pub enum AgentCommand {
    UserMessage(String),
    ToolApproved { call_index: usize },
    ToolDenied { call_index: usize },
    ToolAlwaysApprove,
    ShellInputResponse(String),
    Continue,
    AnswerNow,
    Shutdown,
    ContextSummarized { id: String, summary: String },
}

pub struct AgentLoop {
    client: ApiClient,
    config: AppConfig,
    session: Session,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    command_rx: mpsc::UnboundedReceiver<AgentCommand>,
    command_tx: mpsc::UnboundedSender<AgentCommand>,
    auto_approve: bool,
    iteration: u32,
}

impl AgentLoop {
    pub fn new(
        config: AppConfig,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        command_rx: mpsc::UnboundedReceiver<AgentCommand>,
        command_tx: mpsc::UnboundedSender<AgentCommand>,
        registry: Arc<SkillRegistry>,
    ) -> Self {
        let client = ApiClient::new(&config);
        let system_prompt = build_system_prompt(&config.agent.working_directory);
        let auto_approve = config.agent.auto_approve_tools;

        let session_id = uuid::Uuid::new_v4().to_string();
        let mut session = Session::new(session_id, "New Chat".to_string());
        session.task_manager = session.task_manager.with_sender(event_tx.clone());
        session.tool_registry = Some(registry);
        session.messages.push(ChatMessage::system(&system_prompt));

        Self {
            client,
            config,
            session,
            event_tx,
            command_rx,
            command_tx,
            auto_approve,
            iteration: 0,
        }
    } // new

    pub fn resume(
        config: AppConfig,
        session_id: &str,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        command_rx: mpsc::UnboundedReceiver<AgentCommand>,
        command_tx: mpsc::UnboundedSender<AgentCommand>,
        registry: Arc<SkillRegistry>,
    ) -> Result<Self> {
        let client = ApiClient::new(&config);
        let mut session = Session::load(session_id)?;
        session.task_manager = session.task_manager.with_sender(event_tx.clone());
        session.tool_registry = Some(registry);
        let auto_approve = config.agent.auto_approve_tools;

        Ok(Self {
            client,
            config,
            session,
            event_tx,
            command_rx,
            command_tx,
            auto_approve,
            iteration: 0,
        })
    } // resume

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                Some(command) = self.command_rx.recv() => {
                    match command {
                        AgentCommand::UserMessage(msg) => {
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
                        AgentCommand::ContextSummarized { id, summary } => {
                            let search_str = format!("[Summarizing context segment {}...]", id);
                            if let Some(msg) = self.session.messages.iter_mut().find(|m| m.role == "system" && m.content.as_deref() == Some(search_str.as_str())) {
                                msg.content = Some(format!("--- PAST CONTEXT SUMMARY ---\n{}\n----------------------------", summary));
                                self.session.save().ok();
                                self.event_tx.send(AgentEvent::ContextSummaryReady { id, summary }).ok();
                            }
                        }
                        AgentCommand::Shutdown => break,
                        _ => {}
                    }
                }
            }
        }
    } // run

    async fn run_agent_turn(&mut self) {
        'turn: loop {
            match self.drain_pending_commands().await {
                DrainResult::Shutdown => return,
                DrainResult::UserMessageInjected => {
                    self.event_tx.send(AgentEvent::IterationUpdate(self.iteration)).ok();
                    self.prune_messages();
                }
                DrainResult::Nothing => {}
            }

            if self.iteration >= self.config.agent.max_iterations {
                self.event_tx.send(AgentEvent::MaxIterationsReached).ok();
                match self.wait_for_continue_or_answer().await {
                    ContinueAction::Continue => {
                        self.iteration = 0;
                        self.event_tx.send(AgentEvent::IterationUpdate(self.iteration)).ok();
                        
                        self.session.messages.push(ChatMessage::user(
                            "I have authorized you to continue. You are on a strict step allowance \u{2014} please focus on finishing the task as quickly and efficiently as possible. Avoid unnecessary or repetitive steps."
                        ));
                    }
                    ContinueAction::AnswerNow => {
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

            let threshold = (self.config.agent.max_iterations as f32 * 0.8) as u32;
            if self.iteration == threshold && self.iteration > 1 {
                self.session.messages.push(ChatMessage::user(
                    &format!("--- SYSTEM WARNING: You have reached step {} of {}. Please start wrapping up your work and move toward completion now. ---", self.iteration, self.config.agent.max_iterations)
                ));
            }

            self.prune_messages();

            let registry = match self.session.tool_registry.as_ref() {
                Some(reg) => reg,
                None => {
                    self.event_tx.send(AgentEvent::Error("Tool registry not initialized".to_string())).ok();
                    break;
                }
            };
            let tool_defs = tools::all_tool_definitions(registry);
            let stream_result = self
                .client
                .chat_completion_stream(
                    self.session.messages.clone(),
                    &self.config.current_provider().model,
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

            let mut content_buf = String::new();
            let mut reasoning_buf = String::new();
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
                                reasoning_buf.push_str(&text);
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
                        self.session.messages.push(ChatMessage::user(&msg));
                        self.event_tx.send(AgentEvent::Activity(crate::tools::ActivityEntry {
                            tool_name: "chat".to_string(),
                            summary: "Interrupted by user message".to_string(),
                            status: crate::tools::ActivityStatus::Success,
                            timestamp: chrono::Utc::now(),
                            thread_id: None,
                            total_threads: None,
                        })).ok();
                        
                        if !content_buf.is_empty() {
                             self.session.messages.push(ChatMessage::assistant(&content_buf));
                        }
                        
                        self.iteration = 0;
                        self.session.save().ok();
                        continue 'turn;
                    }
                    else => break,
                }
            }

            if !tool_calls.is_empty() {
                let content = if content_buf.is_empty() { None } else { Some(content_buf.clone()) };
                // DeepSeek reasoner requires reasoning_content to be present when tool_calls are used.
                let reasoning = if self.config.current_provider().model.contains("reasoner") || !reasoning_buf.is_empty() {
                    Some(reasoning_buf.clone())
                } else {
                    None
                };

                self.session.messages.push(ChatMessage::assistant_with_tool_calls(
                    content,
                    reasoning,
                    tool_calls.clone(),
                ));

                let mut join_set = tokio::task::JoinSet::new();
                
                for tc in &tool_calls {
                    self.event_tx.send(AgentEvent::ToolCallStart {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    }).ok();

                    if !self.auto_approve {
                        self.event_tx.send(AgentEvent::ToolApprovalRequest {
                            call_index: tool_calls.iter().position(|t| t.id == tc.id).unwrap_or(0),
                            name: tc.function.name.clone(),
                            arguments: tc.function.arguments.clone(),
                        }).ok();

                        if !self.wait_for_approval().await {
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

                    let name = tc.function.name.clone();
                    let arguments = tc.function.arguments.clone();
                    let id = tc.id.clone();
                    let tm_clone = self.session.task_manager.clone();
                    let registry_clone = match self.session.tool_registry.as_ref() {
                        Some(reg) => reg.clone(),
                        None => {
                            // This should not happen since we checked earlier, but handle gracefully
                            self.event_tx.send(AgentEvent::Error("Tool registry not available".to_string())).ok();
                            continue;
                        }
                    };
                    
                    let thread_id = join_set.len() + 1;
                    let total_threads = tool_calls.len();
                    
                    join_set.spawn(async move {
                        let (result, activity) = tools::execute_tool(
                            &name, 
                            &arguments, 
                            &tm_clone, 
                            &registry_clone, 
                            Some(thread_id), 
                            Some(total_threads)
                        ).await;
                        (id, name, result, activity)
                    });
                }

                while let Some(res) = join_set.join_next().await {
                    if let Ok((id, name, result, activity)) = res {
                        self.event_tx.send(AgentEvent::Activity(activity)).ok();
                        self.event_tx.send(AgentEvent::ToolCallResult {
                            name,
                            result: result.clone(),
                        }).ok();
                        self.session.messages.push(ChatMessage::tool_result(&id, &result));
                    }
                }

                self.session.save().ok();
                continue;
            }

            if !content_buf.is_empty() {
                self.session.messages.push(ChatMessage::assistant(&content_buf));
            }
            self.session.save().ok();
            self.event_tx.send(AgentEvent::TurnComplete).ok();
            break;
        }
    } // run_agent_turn

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
    } // wait_for_approval

    async fn wait_for_continue_or_answer(&mut self) -> ContinueAction {
        while let Some(cmd) = self.command_rx.recv().await {
            match cmd {
                AgentCommand::Continue => return ContinueAction::Continue,
                AgentCommand::AnswerNow => return ContinueAction::AnswerNow,
                AgentCommand::Shutdown => return ContinueAction::Shutdown,
                AgentCommand::UserMessage(msg) => {
                    self.session.messages.push(ChatMessage::user(&msg));
                    return ContinueAction::AnswerNow;
                }
                _ => {}
            }
        }
        ContinueAction::Shutdown
    } // wait_for_continue_or_answer

    async fn do_final_answer(&mut self) {
        self.prune_messages();

        let stream_result = self
            .client
            .chat_completion_stream(
                self.session.messages.clone(),
                &self.config.current_provider().model,
                None,
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
    } // do_final_answer

    fn prune_messages(&mut self) {
        let max_messages = self.config.agent.context_window_threshold;

        if self.session.messages.len() <= max_messages {
            return;
        }

        let mut keep_initial = 5;
        while keep_initial < self.session.messages.len() {
            let msg = &self.session.messages[keep_initial - 1];
            if msg.role == "assistant" && msg.tool_calls.is_some() {
                let mut j = keep_initial;
                while j < self.session.messages.len() && self.session.messages[j].role == "tool" {
                    j += 1;
                }
                keep_initial = j;
            } else {
                break;
            }
        }

        let total = self.session.messages.len();
        let msg_to_keep = self.config.agent.context_window_keep.max(10);
        let mut start = total.saturating_sub(msg_to_keep);

        while start < total {
            let role = self.session.messages[start].role.as_str();
            
            if role == "tool" {
                start += 1;
                continue;
            }
            break;
        }

        if start <= keep_initial {
            return;
        }
        
        // Extract the messages we're removing so we can summarize them
        let messages_to_summarize = self.session.messages[keep_initial..start].to_vec();

        let mut new_messages = Vec::new();
        if self.session.messages.len() > keep_initial {
            new_messages.extend(self.session.messages.iter().take(keep_initial).cloned());
        }

        let summary_id = uuid::Uuid::new_v4().to_string();
        new_messages.push(ChatMessage::system(&format!("[Summarizing context segment {}...]", summary_id)));

        if start < total {
            new_messages.extend(self.session.messages[start..].iter().cloned());
        }
        
        self.session.messages = new_messages;
        self.event_tx.send(AgentEvent::ContextPruned { count: start - keep_initial }).ok();

        // Spawn summarizer task
        let client = self.client.clone();
        let cmd_tx = self.command_tx.clone();
        let model = self.config.current_provider().model.clone();

        tokio::spawn(async move {
            let pt = "You are a highly capable AI agent context summarizer. Your goal is to take a transcript of past conversation history and tool executions, and summarize it accurately so it can serve as a seamless working memory for the agent going forward. Retain all factual information, ongoing tasks, specific file paths mentioned, and critical tool outputs. Ensure the agent knows EXACTLY where it left off. Be highly concise but technically precise.".to_string();
            
            let mut summary_messages = vec![ChatMessage::system(&pt)];
            let mut conversation_text = String::new();
            for m in messages_to_summarize {
                conversation_text.push_str(&format!("{}: {}\n\n", m.role, m.content.as_deref().unwrap_or("[No content]")));
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        conversation_text.push_str(&format!("Tool Call: {}({})\n", tc.function.name, tc.function.arguments));
                    }
                }
            }
            
            summary_messages.push(ChatMessage::user(&format!("Please summarize the following conversation history:\n\n{}", conversation_text)));
            
            if let Ok(summary) = client.chat_completion(summary_messages, &model).await {
                cmd_tx.send(AgentCommand::ContextSummarized { id: summary_id, summary }).ok();
            }
        });
    } // prune_messages

    pub fn task_manager(&self) -> &TaskManager {
        &self.session.task_manager
    } // task_manager

    async fn drain_pending_commands(&mut self) -> DrainResult {
        let mut found_user_message = false;

        loop {
            match self.command_rx.try_recv() {
                Ok(AgentCommand::UserMessage(msg)) => {
                    self.session.messages.push(ChatMessage::user(&msg));
                    found_user_message = true;
                }
                Ok(AgentCommand::Shutdown) => return DrainResult::Shutdown,
                Ok(AgentCommand::ToolAlwaysApprove) => {
                    self.auto_approve = true;
                }
                Ok(AgentCommand::ContextSummarized { id, summary }) => {
                    let search_str = format!("[Summarizing context segment {}...]", id);
                    if let Some(msg) = self.session.messages.iter_mut().find(|m| m.role == "system" && m.content.as_deref() == Some(search_str.as_str())) {
                        msg.content = Some(format!("--- PAST CONTEXT SUMMARY ---\n{}\n----------------------------", summary));
                        self.session.save().ok();
                        self.event_tx.send(AgentEvent::ContextSummaryReady { id, summary }).ok();
                    }
                }
                Ok(_) => {}
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
    } // drain_pending_commands
} // impl AgentLoop

enum ContinueAction {
    Continue,
    AnswerNow,
    Shutdown,
}

enum DrainResult {
    UserMessageInjected,
    Shutdown,
    Nothing,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::ToolCall;

    fn create_test_loop() -> AgentLoop {
        let config = AppConfig::default();
        let (event_tx, _) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let registry = Arc::new(SkillRegistry::new(None));
        AgentLoop::new(config, event_tx, command_rx, command_tx, registry)
    } // create_test_loop

    #[tokio::test]
    async fn test_prune_messages_no_orphaned_tools() {
        let mut agent = create_test_loop();
        
        agent.session.messages.clear();
        agent.session.messages.push(ChatMessage::system("system"));
        for i in 1..105 {
            agent.session.messages.push(ChatMessage::user(&format!("user {}", i)));
        }
        
        let tc_id = "test_id".to_string();
        agent.session.messages.push(ChatMessage::assistant_with_tool_calls(
            None,
            None,
            vec![ToolCall {
                id: tc_id.clone(),
                call_type: "function".to_string(),
                function: crate::api::types::FunctionCall {
                    name: "test_tool".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        ));
        agent.session.messages.push(ChatMessage::tool_result(&tc_id, "result"));

        agent.prune_messages();

        let last_msg = agent.session.messages.last().unwrap_or_else(|| {
            panic!("messages should not be empty");
        });
        assert_eq!(last_msg.role, "tool");
        
        let prev_msg = &agent.session.messages[agent.session.messages.len() - 2];
        assert_eq!(prev_msg.role, "assistant");
        assert!(prev_msg.tool_calls.is_some());
    } // test_prune_messages_no_orphaned_tools

    #[tokio::test]
    async fn test_prune_messages_keep_initial_expansion() {
        let mut agent = create_test_loop();
        
        agent.session.messages.clear();
        agent.session.messages.push(ChatMessage::system("system"));
        agent.session.messages.push(ChatMessage::user("initial user"));
        agent.session.messages.push(ChatMessage::assistant("initial assistant"));
        
        let tc_id = "tc1".to_string();
        agent.session.messages.push(ChatMessage::assistant_with_tool_calls(
            None,
            None,
            vec![ToolCall {
                id: tc_id.clone(),
                call_type: "function".to_string(),
                function: crate::api::types::FunctionCall {
                    name: "tool1".to_string(),
                    arguments: "{}".to_string(),
                },
            }],
        ));
        agent.session.messages.push(ChatMessage::tool_result(&tc_id, "res1"));
        
        for i in 0..120 {
            agent.session.messages.push(ChatMessage::user(&format!("msg {}", i)));
        }

        agent.prune_messages();

        assert_eq!(agent.session.messages[3].role, "assistant");
        assert_eq!(agent.session.messages[4].role, "tool");
    } // test_prune_messages_keep_initial_expansion
} // tests
