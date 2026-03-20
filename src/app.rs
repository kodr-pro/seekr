use anyhow::Result;
use crossterm::event::EventStream;
use ratatui::DefaultTerminal;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use crate::agent::{AgentCommand, AgentEvent};
use crate::api::client::ApiClient;
use crate::config::AppConfig;
use crate::tools::task::Task;
use crate::tools::ActivityEntry;
use crate::ui::render::render;
use crate::event_handler::handle_event;

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Setup,
    Main,
    QuitConfirm,
    AwaitingContinue,
    Help,
    UnifiedMenu,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Input,
    Chat,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SelectionMode {
    #[default]
    Normal,
    Visual,
    VisualLine,
}

#[derive(Debug, Clone)]
pub struct ChatSelection {
    pub vline: usize,
    pub col: usize,
    pub anchor_vline: Option<usize>,
    pub anchor_col: Option<usize>,
    pub mode: SelectionMode,
}

impl Default for ChatSelection {
    fn default() -> Self {
        Self {
            vline: 0,
            col: 0,
            anchor_vline: None,
            anchor_col: None,
            mode: SelectionMode::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub enum InputMode {
    Normal,
    ShellStdin {
        context: String,
        input_tx: tokio::sync::mpsc::UnboundedSender<String>,
    },
}

impl PartialEq for InputMode {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (InputMode::Normal, InputMode::Normal)
                | (InputMode::ShellStdin { .. }, InputMode::ShellStdin { .. })
        )
    }
} // eq

#[derive(Debug, Clone)]
pub enum ChatEntry {
    UserMessage(String),
    AssistantContent(String),
    AssistantStreaming(String),
    Reasoning(String),
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, result: String },
    Error(String),
    SystemInfo(String),
    ToolApproval { name: String, arguments: String },
    CliInputPrompt(String),
    ContextSummary { id: String, summary: String, is_pending: bool },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineType {
    Normal,
    Header,
    CodeBlock,
    CodeBlockStart,
    CodeBlockEnd,
}

#[derive(Debug, Clone)]
pub struct VisualLine {
    pub entry_idx: usize,
    pub text: String,
    pub is_header: bool,
    pub line_type: LineType,
    pub language: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SetupState {
    pub current_step: usize,
    pub provider_selection: usize,
    pub api_key_input: String,
    pub model_selection: usize,
    pub auto_approve_selection: usize,
    pub working_dir_input: String,
    pub error_message: Option<String>,
    pub validating: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuTab {
    Sessions,
    Models,
    Providers,
    Settings,
    Help,
}

#[derive(Debug, Clone)]
pub struct MenuState {
    pub active_tab: MenuTab,
    pub selection_idx: usize,
    pub scroll_offset: usize,
}

#[derive(Debug)]
pub enum BgEvent {
    ModelsFetched(Result<Vec<String>, String>),
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            active_tab: MenuTab::Sessions,
            selection_idx: 0,
            scroll_offset: 0,
        }
    }
}

pub struct App {
    pub mode: AppMode,
    pub focus: Focus,
    pub config: Option<AppConfig>,

    pub chat_entries: Vec<ChatEntry>,
    pub visual_lines: Vec<VisualLine>,
    pub needs_recompute_vlines: bool,
    pub input: String,
    pub cursor_pos: usize,
    pub scroll_offset: u16,
    pub chat_max_scroll: u16,
    pub user_scrolled: bool,
    pub show_reasoning: bool,

    pub agent_cmd_tx: Option<mpsc::UnboundedSender<AgentCommand>>,
    pub agent_event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,

    pub total_tokens: u32,
    pub iteration: u32,
    pub connected: bool,
    pub awaiting_approval: bool,
    pub input_mode: InputMode,

    pub tasks: Vec<Task>,
    pub activities: Vec<ActivityEntry>,
    pub live_activities: Vec<ActivityEntry>,

    pub menu_state: MenuState,

    pub streaming_content: String,
    pub streaming_reasoning: String,
    pub is_streaming: bool,

    pub manager: Option<std::sync::Arc<crate::manager::SeekrManager>>,
    pub setup_state: SetupState,
    pub session_id: Option<String>,

    pub sessions: Vec<crate::session::SessionMetadata>,
    pub session_list_error: Option<String>,

    pub available_models: Vec<String>,
    pub chat_selection: ChatSelection,
    pub last_chat_width: u16,
    pub clipboard: Option<arboard::Clipboard>,
    pub layout: Option<crate::ui::layout::AppLayout>,
    pub terminal_width: u16,
    pub terminal_height: u16,

    pub bg_tx: tokio::sync::mpsc::UnboundedSender<BgEvent>,
    pub bg_rx: tokio::sync::mpsc::UnboundedReceiver<BgEvent>,
}

impl App {
    pub fn new_setup() -> Self {
        let (bg_tx, bg_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            mode: AppMode::Setup,
            focus: Focus::Input,
            config: None,
            manager: None,
            chat_entries: Vec::new(),
            visual_lines: Vec::new(),
            needs_recompute_vlines: true,
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            chat_max_scroll: 0,
            show_reasoning: true,
            user_scrolled: false,
            agent_cmd_tx: None,
            agent_event_rx: None,
            total_tokens: 0,
            iteration: 0,
            connected: false,
            awaiting_approval: false,
            input_mode: InputMode::Normal,
            tasks: Vec::new(),
            activities: Vec::new(),
            live_activities: Vec::new(),
            menu_state: MenuState::default(),
            streaming_content: String::new(),
            streaming_reasoning: String::new(),
            is_streaming: false,
            setup_state: SetupState {
                current_step: 0,
                provider_selection: 0,
                api_key_input: String::new(),
                model_selection: 0,
                auto_approve_selection: 0,
                working_dir_input: String::new(),
                error_message: None,
                validating: false,
            },
            session_id: None,
            sessions: Vec::new(),
            session_list_error: None,
            available_models: Vec::new(),
            chat_selection: ChatSelection::default(),
            last_chat_width: 80,
            layout: None,
            terminal_width: 0,
            terminal_height: 0,
            clipboard: arboard::Clipboard::new().ok(),
            bg_tx,
            bg_rx,
        }
    } // new_setup

    pub fn new_main(config: AppConfig) -> Self {
        let show_reasoning = config.ui.show_reasoning;
        let manager = std::sync::Arc::new(crate::manager::SeekrManager::new(config.clone()));
        let mut app = Self {
            mode: AppMode::Main,
            config: Some(config),
            manager: Some(manager),
            show_reasoning,
            ..Self::new_setup()
        };
        app.chat_entries.push(ChatEntry::SystemInfo(
            "Welcome to Seekr! Type a message to start.".to_string(),
        ));
        app.needs_recompute_vlines = true;
        app
    } // new_main

    pub fn start_agent(&mut self) {
        let config = match self.config.as_ref() {
            Some(c) => c,
            None => return,
        };

        if let Some(ref tx) = self.agent_cmd_tx {
            tx.send(AgentCommand::Shutdown).ok();
        }

        let (evt_tx, evt_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        let agent_res = if let Some(sid) = &self.session_id {
            let registry = match self.manager.as_ref() {
                Some(m) => m.tool_registry(),
                None => {
                    self.chat_entries.push(ChatEntry::Error("Manager not initialized".to_string()));
                    return;
                }
            };
            crate::agent::loop_mod::AgentLoop::resume(config.clone(), sid, evt_tx, cmd_rx, cmd_tx.clone(), registry)
        } else {
            let registry = match self.manager.as_ref() {
                Some(m) => m.tool_registry(),
                None => {
                    self.chat_entries.push(ChatEntry::Error("Manager not initialized".to_string()));
                    return;
                }
            };
            Ok(crate::agent::loop_mod::AgentLoop::new(
                config.clone(),
                evt_tx,
                cmd_rx,
                cmd_tx.clone(),
                registry,
            ))
        };

        match agent_res {
            Ok(agent) => {
                tokio::spawn(agent.run());
                self.agent_cmd_tx = Some(cmd_tx);
                self.agent_event_rx = Some(evt_rx);
                self.connected = true;
            }
            Err(e) => {
                self.chat_entries
                    .push(ChatEntry::Error(format!("Failed to start agent: {}", e)));
            }
        }
    } // start_agent

    pub fn resume_session(&mut self, session_id: String) {
        self.session_id = Some(session_id.clone());
        match crate::session::Session::load(&session_id) {
            Ok(session) => {
                self.chat_entries.clear();
                self.needs_recompute_vlines = true;
                for msg in session.messages {
                    match (msg.role.as_str(), msg.content) {
                        ("user", Some(content)) => {
                            self.chat_entries.push(ChatEntry::UserMessage(content))
                        }
                        ("assistant", content) => {
                            if let Some(c) = content {
                                self.chat_entries.push(ChatEntry::AssistantContent(c));
                            }
                            if let Some(tool_calls) = msg.tool_calls {
                                for tc in tool_calls {
                                    self.chat_entries.push(ChatEntry::ToolCall {
                                        name: tc.function.name,
                                        arguments: tc.function.arguments,
                                    });
                                }
                            }
                        }
                        ("tool", Some(content)) => {
                            self.chat_entries.push(ChatEntry::ToolResult {
                                name: "resumed".to_string(),
                                result: content,
                            });
                        }
                        ("system", Some(content)) => {
                            if content.contains("--- PAST CONTEXT SUMMARY ---") {
                                self.chat_entries.push(ChatEntry::ContextSummary {
                                    id: String::new(),
                                    summary: content.replace("--- PAST CONTEXT SUMMARY ---\n", "").replace("\n----------------------------", ""),
                                    is_pending: false,
                                });
                            } else if content.contains("[Summarizing context segment") {
                                let id = content.split_whitespace().last().unwrap_or("").trim_end_matches("...]").to_string();
                                self.chat_entries.push(ChatEntry::ContextSummary {
                                    id,
                                    summary: "Summarizing past context...".to_string(),
                                    is_pending: true,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                self.tasks = session.task_manager.tasks();
            }
            Err(e) => {
                self.chat_entries
                    .push(ChatEntry::Error(format!("Failed to load session: {}", e)));
            }
        }
    } // resume_session

    pub fn send_message(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }
        let msg = self.input.clone();
        self.input.clear();
        self.cursor_pos = 0;

        if let InputMode::ShellStdin { ref input_tx, .. } = self.input_mode.clone() {
            let _ = input_tx.send(msg);
            self.input_mode = InputMode::Normal;
            self.needs_recompute_vlines = true;
            return;
        }

        self.chat_entries.push(ChatEntry::UserMessage(msg.clone()));
        self.needs_recompute_vlines = true;
        self.is_streaming = true;
        self.streaming_content.clear();
        self.streaming_reasoning.clear();
        self.user_scrolled = false;

        if let Some(ref tx) = self.agent_cmd_tx {
            tx.send(AgentCommand::UserMessage(msg)).ok();
        }

        self.scroll_offset = self.chat_max_scroll;
    } // send_message

    pub fn poll_agent_events(&mut self) {
        let events: Vec<AgentEvent> = {
            let rx = match self.agent_event_rx.as_mut() {
                Some(rx) => rx,
                None => return,
            };
            let mut events = Vec::new();
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
            events
        };

        for event in events {
            match event {
                AgentEvent::ContentDelta(text) => {
                    self.streaming_content.push_str(&text);
                    self.update_streaming_entry();
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::ReasoningDelta(text) => {
                    self.streaming_reasoning.push_str(&text);
                    if self.show_reasoning {
                        self.update_reasoning_entry();
                    }
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::ToolCallStart { name, arguments } => {
                    self.finalize_streaming();
                    self.chat_entries
                        .push(ChatEntry::ToolCall { name, arguments });
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::ToolCallResult { name, result } => {
                    self.chat_entries
                        .push(ChatEntry::ToolResult { name, result });
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::Activity(entry) => {
                    self.activities.push(entry.clone());
                    if let Some(tid) = entry.thread_id {
                        if entry.status == crate::tools::ActivityStatus::Starting {
                            self.live_activities.push(entry);
                        } else {
                            self.live_activities.retain(|a| a.thread_id != Some(tid));
                        }
                    }
                }
                AgentEvent::TokenUsage { total_tokens, .. } => {
                    self.total_tokens = total_tokens;
                }
                AgentEvent::IterationUpdate(n) => {
                    self.iteration = n;
                }
                AgentEvent::TurnComplete => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.iteration = 0;
                    if self.mode == AppMode::AwaitingContinue {
                        self.mode = AppMode::Main;
                    }
                }
                AgentEvent::MaxIterationsReached => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.mode = AppMode::AwaitingContinue;
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::Error(msg) => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.chat_entries.push(ChatEntry::Error(msg));
                }
                AgentEvent::ToolApprovalRequest {
                    name, arguments, ..
                } => {
                    self.awaiting_approval = true;
                    self.chat_entries
                        .push(ChatEntry::ToolApproval { name, arguments });
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::ShellInputNeeded { context, input_tx } => {
                    self.input_mode = InputMode::ShellStdin { context, input_tx };
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::TaskCreated(task) | AgentEvent::TaskUpdated(task) => {
                    if let Some(pos) = self.tasks.iter().position(|t| t.id == task.id) {
                        self.tasks[pos] = task;
                    } else {
                        self.tasks.push(task);
                    }
                }
                AgentEvent::ContextPruned { count } => {
                    // Try to find where to remove messages. 
                    // Usually we keep the first few SystemInfo/Welcome messages.
                    let start_idx = self.chat_entries.iter().position(|e| !matches!(e, ChatEntry::SystemInfo(_))).unwrap_or(0);
                    let mut removed = 0;
                    let mut i = start_idx;
                    while removed < count && i < self.chat_entries.len() {
                        if matches!(self.chat_entries[i], ChatEntry::UserMessage(_) | ChatEntry::AssistantContent(_) | ChatEntry::ToolCall { .. } | ChatEntry::ToolResult { .. } | ChatEntry::Reasoning(_)) {
                            self.chat_entries.remove(i);
                            removed += 1;
                        } else {
                            i += 1;
                        }
                    }
                    // The placeholder will be handled by the next message or we can add it here if we had the ID.
                    // But AgentLoop emits ContextPruned, and it already added the placeholder to its session.
                    // Let's rely on the fact that we'll get a ContextSummaryReady event soon with the same ID.
                }
                AgentEvent::ContextSummaryReady { id, summary } => {
                    if let Some(pos) = self.chat_entries.iter().rposition(|e| match e { 
                        ChatEntry::ContextSummary { id: s_id, .. } => s_id == &id,
                        _ => false 
                    }) {
                         if let ChatEntry::ContextSummary { summary: ref mut s, is_pending: ref mut p, .. } = self.chat_entries[pos] {
                             *s = summary;
                             *p = false;
                         }
                    } else {
                        self.chat_entries.push(ChatEntry::ContextSummary { id, summary, is_pending: false });
                    }
                    self.needs_recompute_vlines = true;
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
            }
        }
    } // poll_agent_events

    fn update_streaming_entry(&mut self) {
        if let Some(ChatEntry::AssistantStreaming(ref mut content)) = self.chat_entries.last_mut() {
            *content = self.streaming_content.clone();
            return;
        }
        self.chat_entries.push(ChatEntry::AssistantStreaming(
            self.streaming_content.clone(),
        ));
    } // end update_streaming_entry

    fn update_reasoning_entry(&mut self) {
        let len = self.chat_entries.len();
        for i in (0..len).rev() {
            if let ChatEntry::Reasoning(ref mut text) = self.chat_entries[i] {
                *text = self.streaming_reasoning.clone();
                return;
            }
            if !matches!(
                &self.chat_entries[i],
                ChatEntry::AssistantStreaming(_) | ChatEntry::Reasoning(_)
            ) {
                break;
            }
        }

        let insert_pos = if matches!(
            self.chat_entries.last(),
            Some(ChatEntry::AssistantStreaming(_))
        ) {
            self.chat_entries.len() - 1
        } else {
            self.chat_entries.len()
        };
        self.chat_entries.insert(
            insert_pos,
            ChatEntry::Reasoning(self.streaming_reasoning.clone()),
        );
        self.needs_recompute_vlines = true;
    } // end update_reasoning_entry

    fn finalize_streaming(&mut self) {
        if !self.streaming_content.is_empty() {
            if let Some(pos) = self
                .chat_entries
                .iter()
                .rposition(|e| matches!(e, ChatEntry::AssistantStreaming(_)))
            {
                self.chat_entries[pos] =
                    ChatEntry::AssistantContent(self.streaming_content.clone());
            } else {
                self.chat_entries
                    .push(ChatEntry::AssistantContent(self.streaming_content.clone()));
            }
            self.streaming_content.clear();
        }
        self.streaming_reasoning.clear();
    } // finalize_streaming

    pub fn handle_approval(&mut self, approved: bool, always: bool) {
        self.awaiting_approval = false;
        if let Some(pos) = self
            .chat_entries
            .iter()
            .rposition(|e| matches!(e, ChatEntry::ToolApproval { .. }))
        {
            self.chat_entries.remove(pos);
        }

        if let Some(ref tx) = self.agent_cmd_tx {
            if always {
                tx.send(AgentCommand::ToolAlwaysApprove).ok();
            } else if approved {
                tx.send(AgentCommand::ToolApproved { call_index: 0 }).ok();
            } else {
                tx.send(AgentCommand::ToolDenied { call_index: 0 }).ok();
            }
        }
    } // handle_approval

    pub fn clear_chat(&mut self) {
        self.chat_entries.clear();
        self.chat_entries.push(ChatEntry::SystemInfo(
            "Chat cleared. Type a message to continue.".to_string(),
        ));
        self.scroll_offset = 0;
        self.needs_recompute_vlines = true;
    } // clear_chat

    pub async fn load_sessions(&mut self) {
        if let Some(ref mgr) = self.manager {
            match mgr.load_sessions().await {
                Ok(_) => {
                    self.sessions = mgr.list_sessions().await;
                    self.session_list_error = None;
                }
                Err(e) => {
                    self.session_list_error = Some(format!("Failed to load sessions: {e}"));
                    self.sessions.clear();
                }
            }
        } else {
            match crate::session::Session::list_all() {
                Ok(s) => {
                    self.sessions = s;
                    self.session_list_error = None;
                }
                Err(e) => {
                    self.session_list_error = Some(format!("Failed to load sessions: {e}"));
                    self.sessions.clear();
                }
            }
        }
    } // load_sessions

    pub fn poll_bg_events(&mut self) {
        while let Ok(event) = self.bg_rx.try_recv() {
            match event {
                BgEvent::ModelsFetched(Ok(models)) => {
                    self.available_models = models;
                }
                BgEvent::ModelsFetched(Err(e)) => {
                    self.chat_entries
                        .push(ChatEntry::Error(format!("Failed to fetch models: {}", e)));
                }
            }
        }
    } // poll_bg_events

    pub fn fetch_available_models(&self) {
        if self.config.is_none() {
            return;
        }
        let config = match self.config.clone() {
            Some(c) => c,
            None => return,
        };
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let client = ApiClient::new(&config);
            let res = client.list_models().await.map_err(|e| e.to_string());
            let _ = tx.send(BgEvent::ModelsFetched(res));
        });
    } // fetch_available_models

    pub async fn open_unified_menu(&mut self) {
        self.load_sessions().await;
        if self.available_models.is_empty() {
            self.fetch_available_models();
        }
        self.mode = AppMode::UnifiedMenu;
        self.menu_state = MenuState::default();
    } // open_unified_menu

    pub async fn delete_session_at(&mut self, idx: usize) {
        let session = match self.sessions.get(idx) {
            Some(s) => s,
            None => return,
        };
        let id = session.id.clone();

        if let Some(ref mgr) = self.manager {
            if let Err(e) = mgr.delete_session(&id).await {
                self.session_list_error = Some(format!("Failed to delete session: {e}"));
                return;
            }
            self.sessions = mgr.list_sessions().await;
        } else {
            if let Ok(dir) = crate::session::Session::sessions_dir() {
                let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
            }
            self.sessions.retain(|s| s.id != id);
        }
    } // delete_session_at
    pub fn calculate_visual_lines(&self, width: u16) -> Vec<VisualLine> {
        let mut visual_lines = Vec::new();
        if width == 0 { return visual_lines; }

        for (idx, entry) in self.chat_entries.iter().enumerate() {
            let (header, content) = match entry {
                ChatEntry::UserMessage(msg) => (Some("[YOU]"), msg.as_str()),
                ChatEntry::AssistantContent(text) | ChatEntry::AssistantStreaming(text) => (Some("[SEEKR]"), if text.is_empty() && matches!(entry, ChatEntry::AssistantStreaming(_)) { "..." } else { text.as_str() }),
                ChatEntry::Reasoning(text) => (Some("[THINKING]"), text.as_str()),
                ChatEntry::ToolCall { name, arguments } => {
                    let args_short = if arguments.len() > 64 { format!("{}...", &arguments[..64]) } else { arguments.clone() };
                    (None, &*format!("➞ Tool Call: {} ({})", name, args_short))
                }
                ChatEntry::ToolResult { name, result } => {
                     let max_len = 2000;
                     let display_result = if result.len() > max_len { format!("{}... (truncated)", &result[..max_len]) } else { result.clone() };
                     (None, &*format!("✓ Tool Result: {}\n{}", name, display_result))
                }
                ChatEntry::Error(msg) => (Some("[ERROR]"), msg.as_str()),
                ChatEntry::SystemInfo(msg) => (None, &*format!("[INFO] {}", msg)),
                ChatEntry::ToolApproval { name, arguments } => (Some("[APPROVAL REQUIRED]"), &*format!("Agent wants to execute: {}({})\n  [Y]es / [N]o / [A]lways", name, arguments)),
                ChatEntry::CliInputPrompt(prompt) => (Some("[INPUT REQUIRED]"), prompt.as_str()),
                ChatEntry::ContextSummary { summary, is_pending, .. } => {
                    if *is_pending {
                        (Some("[CONTEXT WINDOW]"), "Summarizing past conversation to free up context space...")
                    } else {
                        (Some("[CONTEXT SUMMARY]"), summary.as_str())
                    }
                }
            };

            if idx > 0 {
                visual_lines.push(VisualLine { entry_idx: idx, text: String::new(), is_header: false, line_type: LineType::Normal, language: None });
            }

            if let Some(h) = header {
                visual_lines.push(VisualLine { entry_idx: idx, text: h.to_string(), is_header: true, line_type: LineType::Normal, language: None });
            }

            let mut in_code_block = false;
            let mut current_language: Option<String> = None;
            for line in content.lines() {
                let trimmed = line.trim();
                let is_fence_start = trimmed.starts_with("```");
                let mut line_type = LineType::Normal;
                let mut language: Option<String> = None;
                
                if is_fence_start {
                    if !in_code_block {
                        line_type = LineType::CodeBlockStart;
                        // Extract language from fence start
                        let after_backticks = &trimmed[3..].trim();
                        if !after_backticks.is_empty() {
                            // Take the first word as language
                            let lang = after_backticks.split_whitespace().next().unwrap_or("");
                            if !lang.is_empty() {
                                current_language = Some(lang.to_string());
                            } else {
                                current_language = None;
                            }
                        } else {
                            current_language = None;
                        }
                        language = current_language.clone();
                        // Add copy hint to the fence line
                        // We'll modify the line variable to include the hint
                        // But we need to be careful: line variable is the original line
                        // We'll handle this later in the line wrapping logic
                    } else {
                        line_type = LineType::CodeBlockEnd;
                        // Reset language for fence end line
                        current_language = None;
                    }
                    in_code_block = !in_code_block;
                } else if in_code_block {
                    line_type = LineType::CodeBlock;
                    language = current_language.clone();
                }
                
                // Add copy icon to code block start line
                let line_to_wrap = if line_type == LineType::CodeBlockStart {
                    format!("{} [COPY]", line)
                } else {
                    line.to_string()
                };

                if line.is_empty() {
                    visual_lines.push(VisualLine { entry_idx: idx, text: String::new(), is_header: false, line_type, language });
                    continue;
                }

                let words = line_to_wrap.split_inclusive(' ');
                let mut current_line = String::new();
                for word in words {
                    if current_line.chars().count() + word.chars().count() > width as usize {
                        if !current_line.is_empty() {
                            visual_lines.push(VisualLine { entry_idx: idx, text: current_line.clone(), is_header: false, line_type, language: language.clone() });
                            current_line.clear();
                        }
                        
                        // Handle extremely long words
                        let mut remaining_word = word;
                        while remaining_word.chars().count() > width as usize {
                            let (head, tail) = remaining_word.split_at(remaining_word.char_indices().map(|(i, _)| i).nth(width as usize).unwrap_or(remaining_word.len()));
                            visual_lines.push(VisualLine { entry_idx: idx, text: head.to_string(), is_header: false, line_type, language: language.clone() });
                            remaining_word = tail;
                        }
                        current_line.push_str(remaining_word);
                    } else {
                        current_line.push_str(word);
                    }
                }
                if !current_line.is_empty() {
                    visual_lines.push(VisualLine { entry_idx: idx, text: current_line, is_header: false, line_type, language });
                }
            }
        }
        visual_lines
    } // calculate_visual_lines

    pub fn get_max_vline(&self) -> usize {
        self.calculate_visual_lines(self.last_chat_width).len().saturating_sub(1)
    }

    pub fn get_vline_char_count(&self, vline_idx: usize) -> usize {
        let vlines = self.calculate_visual_lines(self.last_chat_width);
        vlines.get(vline_idx).map(|l| l.text.chars().count()).unwrap_or(0)
    }

    pub fn ensure_vline_visible(&mut self) {
        // Simple logic to adjust scroll_offset so vline is visible
        let visible_height = 20; // Assume some height, or adjust as needed
        if self.chat_selection.vline < self.scroll_offset as usize {
            self.scroll_offset = self.chat_selection.vline as u16;
        } else if self.chat_selection.vline >= (self.scroll_offset + visible_height) as usize {
            self.scroll_offset = (self.chat_selection.vline.saturating_sub(visible_height as usize - 1)) as u16;
        }
    }

    pub fn get_selected_text(&self) -> Option<String> {
        let vlines = self.calculate_visual_lines(self.last_chat_width);
        let anchor_v = self.chat_selection.anchor_vline?;
        let anchor_c = self.chat_selection.anchor_col.unwrap_or(0);
        let cur_v = self.chat_selection.vline;
        let cur_c = self.chat_selection.col;

        let (start_v, start_c, end_v, end_c) = if (anchor_v, anchor_c) <= (cur_v, cur_c) {
            (anchor_v, anchor_c, cur_v, cur_c)
        } else {
            (cur_v, cur_c, anchor_v, anchor_c)
        };

        if self.chat_selection.mode == SelectionMode::VisualLine {
            let mut result = String::new();
            for i in start_v..=end_v {
                if let Some(line) = vlines.get(i) {
                    if !line.is_header {
                        result.push_str(&line.text);
                        result.push('\n');
                    }
                }
            }
            Some(result)
        } else {
            let mut result = String::new();
            for i in start_v..=end_v {
                if let Some(line) = vlines.get(i) {
                    if line.is_header { continue; }
                    let chars: Vec<char> = line.text.chars().collect();
                    let line_start = if i == start_v { start_c } else { 0 };
                    let line_end = if i == end_v { end_c.min(chars.len().saturating_sub(1)) } else { chars.len().saturating_sub(1) };
                    
                    if line_start <= line_end && line_start < chars.len() {
                        for c in &chars[line_start..=line_end] {
                            result.push(*c);
                        }
                    }
                    if i < end_v {
                        result.push('\n');
                    }
                }
            }
            Some(result)
        }
    }
    pub fn copy_code_block_at_vline(&mut self, vline_idx: usize) -> Option<String> {
        let vlines = self.calculate_visual_lines(self.last_chat_width);
        let vline = vlines.get(vline_idx)?;
        // If not part of a code block, return None
        if !matches!(vline.line_type, LineType::CodeBlock | LineType::CodeBlockStart | LineType::CodeBlockEnd) {
            return None;
        }
        // Find start and end indices of the code block
        // First, find the start (CodeBlockStart) before this line
        let mut start = vline_idx;
        while start > 0 && !matches!(vlines[start].line_type, LineType::CodeBlockStart) {
            start -= 1;
        }
        // Ensure we found a start
        if !matches!(vlines[start].line_type, LineType::CodeBlockStart) {
            return None;
        }
        // Find the end (CodeBlockEnd) after start
        let mut end = start;
        while end < vlines.len() && !matches!(vlines[end].line_type, LineType::CodeBlockEnd) {
            end += 1;
        }
        if end >= vlines.len() || !matches!(vlines[end].line_type, LineType::CodeBlockEnd) {
            return None;
        }
        // Collect lines between start+1 and end-1
        let mut code_lines = Vec::new();
        for i in start+1..end {
            code_lines.push(vlines[i].text.clone());
        }
        let code_text = code_lines.join("\n");
        Some(code_text)
    }
} // impl App


pub async fn run_app(mut app: App) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);

    if app.mode == AppMode::Main {
        app.start_agent();
    }

    let result = event_loop(&mut terminal, &mut app).await;

    let _ = ratatui::crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();

    if let Some(ref tx) = app.agent_cmd_tx {
        tx.send(AgentCommand::Shutdown).ok();
    }

    result
} // run_app

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut reader = EventStream::new();
    let mut heartbeat = tokio::time::interval(Duration::from_millis(50));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| render(frame, app))?;

        tokio::select! {
            _ = heartbeat.tick() => {
                // Heartbeat: process background events
                app.poll_bg_events();
                if app.mode == AppMode::Main || app.mode == AppMode::AwaitingContinue {
                    app.poll_agent_events();
                }
            }
            maybe_ev = reader.next() => {
                if let Some(Ok(ev)) = maybe_ev {
                    if handle_event(app, &ev).await? {
                        return Ok(());
                    }
                }
                // Also poll agent events on any user interaction for maximum responsiveness
                app.poll_bg_events();
                if app.mode == AppMode::Main || app.mode == AppMode::AwaitingContinue {
                    app.poll_agent_events();
                }
            }
        }
    }
} // event_loop
