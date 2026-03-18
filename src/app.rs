use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::agent::{AgentCommand, AgentEvent};
use crate::api::client::ApiClient;
use crate::config::AppConfig;
use crate::tools::task::Task;
use crate::tools::ActivityEntry;
use crate::ui;

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

#[derive(Debug, Clone)]
pub struct VisualLine {
    pub entry_idx: usize,
    pub text: String,
    pub is_header: bool,
}

#[derive(Debug, Clone)]
pub struct SetupState {
    pub current_step: usize,
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
            let registry = self.manager.as_ref().unwrap().tool_registry();
            crate::agent::loop_mod::AgentLoop::resume(config.clone(), sid, evt_tx, cmd_rx, cmd_tx.clone(), registry)
        } else {
            let registry = self.manager.as_ref().unwrap().tool_registry();
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
            return;
        }

        self.chat_entries.push(ChatEntry::UserMessage(msg.clone()));
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
        let config = self.config.clone().unwrap();
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
                visual_lines.push(VisualLine { entry_idx: idx, text: String::new(), is_header: false });
            }

            if let Some(h) = header {
                visual_lines.push(VisualLine { entry_idx: idx, text: h.to_string(), is_header: true });
            }

            for line in content.lines() {
                if line.is_empty() {
                    visual_lines.push(VisualLine { entry_idx: idx, text: String::new(), is_header: false });
                    continue;
                }

                let words = line.split_inclusive(' ');
                let mut current_line = String::new();
                for word in words {
                    if current_line.chars().count() + word.chars().count() > width as usize {
                        if !current_line.is_empty() {
                            visual_lines.push(VisualLine { entry_idx: idx, text: current_line.clone(), is_header: false });
                            current_line.clear();
                        }
                        
                        // Handle extremely long words
                        let mut remaining_word = word;
                        while remaining_word.chars().count() > width as usize {
                            let (head, tail) = remaining_word.split_at(remaining_word.char_indices().map(|(i, _)| i).nth(width as usize).unwrap_or(remaining_word.len()));
                            visual_lines.push(VisualLine { entry_idx: idx, text: head.to_string(), is_header: false });
                            remaining_word = tail;
                        }
                        current_line.push_str(remaining_word);
                    } else {
                        current_line.push_str(word);
                    }
                }
                if !current_line.is_empty() {
                    visual_lines.push(VisualLine { entry_idx: idx, text: current_line, is_header: false });
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
    loop {
        terminal.draw(|frame| render(frame, app))?;

        app.poll_bg_events();

        if app.mode == AppMode::Main || app.mode == AppMode::AwaitingContinue {
            app.poll_agent_events();
        }

        if event::poll(Duration::from_millis(16))? {
            let ev = event::read()?;
            match app.mode {
                AppMode::Setup => {
                    if handle_setup_event(app, &ev).await? {
                        return Ok(());
                    }
                }
                AppMode::Main | AppMode::AwaitingContinue => {
                    if handle_main_event(app, &ev).await {
                        return Ok(());
                    }
                }
                AppMode::QuitConfirm => {
                    if handle_quit_confirm(app, &ev) {
                        return Ok(());
                    }
                }
                AppMode::Help => {
                    if let Event::Key(key) = &ev {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => app.mode = AppMode::Main,
                            _ => app.mode = AppMode::Main,
                        }
                    }
                }
                AppMode::UnifiedMenu => {
                    if let Event::Key(key) = &ev {
                        handle_unified_menu_event(app, key).await;
                    }
                }
            }
        }
    }
} // event_loop

async fn handle_unified_menu_event(app: &mut App, key: &KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = AppMode::Main,
        KeyCode::Tab | KeyCode::Char('l') | KeyCode::Right => {
            app.menu_state.active_tab = match app.menu_state.active_tab {
                MenuTab::Sessions => MenuTab::Models,
                MenuTab::Models => MenuTab::Providers,
                MenuTab::Providers => MenuTab::Settings,
                MenuTab::Settings => MenuTab::Help,
                MenuTab::Help => MenuTab::Sessions,
            };
            app.menu_state.selection_idx = 0;
            app.menu_state.scroll_offset = 0;
        }
        KeyCode::Char('h') | KeyCode::Left => {
            app.menu_state.active_tab = match app.menu_state.active_tab {
                MenuTab::Sessions => MenuTab::Help,
                MenuTab::Models => MenuTab::Sessions,
                MenuTab::Providers => MenuTab::Models,
                MenuTab::Settings => MenuTab::Providers,
                MenuTab::Help => MenuTab::Settings,
            };
            app.menu_state.selection_idx = 0;
            app.menu_state.scroll_offset = 0;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_state.selection_idx = app.menu_state.selection_idx.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let max = match app.menu_state.active_tab {
                MenuTab::Sessions => app.sessions.len(),
                MenuTab::Models => app.available_models.len(),
                MenuTab::Providers => app.config.as_ref().map(|c| c.providers.len()).unwrap_or(0),
                MenuTab::Settings => 5, // Hardcoded for now
                MenuTab::Help => 0,
            };
            if app.menu_state.selection_idx + 1 < max {
                app.menu_state.selection_idx += 1;
            }
        }
        KeyCode::Enter => {
            match app.menu_state.active_tab {
                MenuTab::Sessions => {
                    if let Some(session) = app.sessions.get(app.menu_state.selection_idx) {
                        let id = session.id.clone();
                        app.session_id = Some(id.clone());
                        app.mode = AppMode::Main;
                        app.resume_session(id);
                        app.start_agent();
                    }
                }
                MenuTab::Models => {
                    if let Some(model) = app.available_models.get(app.menu_state.selection_idx) {
                        let model_clone = model.clone();
                        if let Some(cfg) = app.config.as_mut() {
                            cfg.current_provider_mut().model = model_clone.clone();
                            cfg.save().ok();
                            app.mode = AppMode::Main;
                            app.chat_entries.push(ChatEntry::SystemInfo(format!(
                                "Switched to model: {}",
                                model_clone
                            )));
                            app.start_agent();
                        }
                    }
                }
                MenuTab::Providers => {
                    if let Some(cfg) = app.config.as_mut() {
                        cfg.active_provider = app.menu_state.selection_idx;
                        cfg.save().ok();
                        app.mode = AppMode::Main;
                        app.chat_entries.push(ChatEntry::SystemInfo(format!(
                            "Switched to provider: {}",
                            cfg.current_provider().name
                        )));
                        app.start_agent();
                    }
                }
                MenuTab::Settings => {
                    if let Some(cfg) = app.config.as_mut() {
                        match app.menu_state.selection_idx {
                            0 => { /* Working Dir - could be complex to edit here */ }
                            1 => {
                                cfg.agent.max_iterations = match cfg.agent.max_iterations {
                                    15 => 30,
                                    30 => 50,
                                    50 => 100,
                                    100 => 200,
                                    200 => 500,
                                    500 => 1000,
                                    _ => 15,
                                };
                            }
                            2 => {
                                cfg.agent.auto_approve_tools = !cfg.agent.auto_approve_tools;
                            }
                            4 => {
                                cfg.ui.show_reasoning = !cfg.ui.show_reasoning;
                                app.show_reasoning = cfg.ui.show_reasoning;
                            }
                            _ => {}
                        }
                        cfg.save().ok();
                    }
                }
                _ => {}
            }
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            if app.menu_state.active_tab == MenuTab::Sessions {
                app.delete_session_at(app.menu_state.selection_idx).await;
            }
        }
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.fetch_available_models();
        }
        _ => {}
    }
} // handle_unified_menu_event

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    match app.mode {
        AppMode::Setup => ui::setup::render_setup(frame, area, &app.setup_state),
        AppMode::Main
        | AppMode::QuitConfirm
        | AppMode::AwaitingContinue
        | AppMode::Help
        | AppMode::UnifiedMenu => {
            render_main(frame, area, app);
            match app.mode {
                AppMode::QuitConfirm => render_quit_dialog(frame, area),
                AppMode::AwaitingContinue => render_continue_dialog(frame, area),
                AppMode::Help => render_help_dialog(frame, area),
                AppMode::UnifiedMenu => ui::menu::render_menu(frame, area, app),
                _ => {}
            }
        }
    }
} // render

fn render_main(frame: &mut Frame, area: Rect, app: &mut App) {
    let layout = ui::layout::AppLayout::new(area);

    render_title_bar(frame, layout.title_bar, app);

    let inner_chat = layout.chat_panel.inner(ratatui::layout::Margin { vertical: 1, horizontal: 1 });
    let visual_lines = app.calculate_visual_lines(inner_chat.width.saturating_sub(2));

    app.chat_max_scroll = ui::chat::render_chat(
        frame,
        layout.chat_panel,
        &visual_lines,
        app.scroll_offset,
        app.focus == Focus::Chat,
        &app.chat_selection,
    );

    ui::tasks::render_tasks(
        frame,
        layout.task_panel,
        &app.tasks,
        &app.activities,
        &app.live_activities,
        app.focus == Focus::Tasks,
    );

    let (shell_context, input_prompt) = match &app.input_mode {
        InputMode::ShellStdin { context, .. } => (
            if context.is_empty() {
                None
            } else {
                Some(context.as_str())
            },
            Some("Shell input required"),
        ),
        InputMode::Normal => (None, None),
    };

    ui::input::render_input(
        frame,
        layout.input_bar,
        &app.input,
        app.cursor_pos,
        app.focus == Focus::Input && !app.awaiting_approval,
        input_prompt,
        shell_context,
    );

    let model = app
        .config
        .as_ref()
        .map(|c| c.current_provider().model.as_str())
        .unwrap_or("unknown");
    let provider = app
        .config
        .as_ref()
        .map(|c| c.current_provider().name.as_str())
        .unwrap_or("unknown");
    let max_iter = app
        .config
        .as_ref()
        .map(|c| c.agent.max_iterations)
        .unwrap_or(15);
    ui::status::render_status(
        frame,
        layout.status_bar,
        &ui::status::StatusInfo {
            session_id: app.session_id.as_deref().unwrap_or("none"),
            connected: app.connected,
            provider,
            model,
            total_tokens: app.total_tokens,
            iteration: app.iteration,
            max_iterations: max_iter,
            is_thinking: app.is_streaming,
        },
    );
} // render_main

fn render_title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let model = app
        .config
        .as_ref()
        .map(|c| c.current_provider().model.as_str())
        .unwrap_or("unknown");
    let status = if app.is_streaming { "Working" } else { "Ready" };

    ui::title::render_title(
        frame,
        area,
        &ui::title::TitleInfo {
            version: "0.1.1",
            session_id: app.session_id.as_deref(),
            connected: app.connected,
            model,
            status,
        },
    );
} // render_title_bar

fn render_quit_dialog(frame: &mut Frame, area: Rect) {
    let dialog_area = centered_rect(44, 7, area);
    frame.render_widget(ratatui::widgets::Clear, dialog_area);

    let block = ratatui::widgets::Block::default()
        .title(" Confirmation ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "    Are you sure you want to quit?",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("    Press "),
            Span::styled(
                "[Y]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to Yes, "),
            Span::styled(
                "[N]",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to No"),
        ]),
    ];

    frame.render_widget(Paragraph::new(text).block(block), dialog_area);
} // render_quit_dialog

fn render_continue_dialog(frame: &mut Frame, area: Rect) {
    let dialog_area = centered_rect(58, 8, area);
    frame.render_widget(ratatui::widgets::Clear, dialog_area);

    let block = ratatui::widgets::Block::default()
        .title(" Max Iterations Reached ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  The agent has used all available iterations.",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled(
                "[C]",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to Continue   "),
            Span::styled(
                "[A]",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to Answer Now"),
        ]),
    ];

    frame.render_widget(Paragraph::new(text).block(block), dialog_area);
} // render_continue_dialog

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
} // centered_rect

async fn handle_setup_event(app: &mut App, ev: &Event) -> Result<bool> {
    if let Event::Key(key) = ev {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Ok(true);
        }

        match app.setup_state.current_step {
            0 => {
                if key.code == KeyCode::Enter {
                    app.setup_state.current_step = 1;
                }
            }
            1 => match key.code {
                KeyCode::Enter => {
                    if !app.setup_state.api_key_input.is_empty() {
                        app.setup_state.error_message = None;
                        app.setup_state.current_step = 2;
                    } else {
                        app.setup_state.error_message = Some("API key cannot be empty".to_string());
                    }
                }
                KeyCode::Esc => app.setup_state.current_step = 0,
                KeyCode::Backspace => {
                    app.setup_state.api_key_input.pop();
                }
                KeyCode::Char(c) => {
                    app.setup_state.api_key_input.push(c);
                    app.setup_state.error_message = None;
                }
                _ => {}
            },
            2 => match key.code {
                KeyCode::Up => {
                    app.setup_state.model_selection =
                        app.setup_state.model_selection.saturating_sub(1)
                }
                KeyCode::Down => {
                    app.setup_state.model_selection = (app.setup_state.model_selection + 1).min(3)
                }
                KeyCode::Enter => app.setup_state.current_step = 3,
                KeyCode::Esc => app.setup_state.current_step = 1,
                _ => {}
            },
            3 => match key.code {
                KeyCode::Up => {
                    app.setup_state.auto_approve_selection =
                        app.setup_state.auto_approve_selection.saturating_sub(1)
                }
                KeyCode::Down => {
                    app.setup_state.auto_approve_selection =
                        (app.setup_state.auto_approve_selection + 1).min(1)
                }
                KeyCode::Enter => app.setup_state.current_step = 4,
                KeyCode::Esc => app.setup_state.current_step = 2,
                _ => {}
            },
            4 => match key.code {
                KeyCode::Enter => {
                    app.setup_state.current_step = 5;
                    app.setup_state.error_message = None;
                    app.setup_state.validating = true;

                    let key = app.setup_state.api_key_input.clone();
                    // Try validating against OpenAI by default in setup wizard
                    let valid =
                        ApiClient::validate_key(&key, "https://api.openai.com/v1", "gpt-4o-mini")
                            .await;
                    app.setup_state.validating = false;

                    match valid {
                        Ok(true) => {
                            let model = match app.setup_state.model_selection {
                                0 => "gpt-4o",
                                1 => "gpt-4o-mini",
                                2 => "claude-3-5-sonnet-latest",
                                3 => "deepseek-chat",
                                _ => "gpt-4o",
                            };
                            let auto_approve = app.setup_state.auto_approve_selection == 1;
                            let working_dir = if app.setup_state.working_dir_input.is_empty() {
                                ".".to_string()
                            } else {
                                app.setup_state.working_dir_input.clone()
                            };

                            let config = AppConfig {
                                providers: vec![crate::config::ProviderConfig {
                                    name: "AI Provider".to_string(),
                                    key: app.setup_state.api_key_input.clone(),
                                    base_url: "https://api.openai.com/v1".to_string(),
                                    model: model.to_string(),
                                }],
                                active_provider: 0,
                                    agent: crate::config::AgentConfig {
                                        max_iterations: 15,
                                        auto_approve_tools: auto_approve,
                                        working_directory: working_dir,
                                        context_window_threshold: 40,
                                        context_window_keep: 10,
                                    },
                                ui: crate::config::UiConfig {
                                    theme: "dark".to_string(),
                                    show_reasoning: true,
                                },
                            };

                            if let Err(e) = config.save() {
                                app.setup_state.error_message =
                                    Some(format!("Failed to save config: {e}"));
                            } else {
                                app.manager = Some(std::sync::Arc::new(
                                    crate::manager::SeekrManager::new(config.clone()),
                                ));
                                app.config = Some(config);
                                app.setup_state.current_step = 6;
                            }
                        }
                        Ok(false) => {
                            app.setup_state.error_message = Some("Invalid API key.".to_string());
                        }
                        Err(e) => {
                            app.setup_state.error_message = Some(format!("Connection error: {e}"));
                        }
                    }
                }
                KeyCode::Esc => app.setup_state.current_step = 3,
                KeyCode::Backspace => {
                    app.setup_state.working_dir_input.pop();
                }
                KeyCode::Char(c) => {
                    app.setup_state.working_dir_input.push(c);
                }
                _ => {}
            },
            5 => {
                if key.code == KeyCode::Enter {
                    app.setup_state.current_step = 1;
                    app.setup_state.error_message = None;
                }
            }
            6 => {
                if key.code == KeyCode::Enter {
                    app.mode = AppMode::Main;
                    app.show_reasoning = true;
                    app.chat_entries
                        .push(ChatEntry::SystemInfo("Welcome to Seekr!".to_string()));
                    app.start_agent();
                }
            }
            _ => {}
        }
    }
    Ok(false)
} // handle_setup_event

pub async fn handle_main_event(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(KeyEvent {
        code, modifiers, ..
    }) = ev
    {
        if *code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            return true;
        }

        if app.mode == AppMode::Help {
            app.mode = AppMode::Main;
            return false;
        }

        if app.mode == AppMode::AwaitingContinue {
            match code {
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    app.mode = AppMode::Main;
                    app.is_streaming = true;
                    app.user_scrolled = false;
                    if let Some(ref tx) = app.agent_cmd_tx {
                        tx.send(AgentCommand::Continue).ok();
                    }
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    app.mode = AppMode::Main;
                    if let Some(ref tx) = app.agent_cmd_tx {
                        tx.send(AgentCommand::AnswerNow).ok();
                    }
                }
                _ => {}
            }
            return false;
        }

        if app.awaiting_approval {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => app.handle_approval(true, false),
                KeyCode::Char('n') | KeyCode::Char('N') => app.handle_approval(false, false),
                KeyCode::Char('a') | KeyCode::Char('A') => app.handle_approval(true, true),
                _ => {}
            }
            return false;
        }

        match code {
            KeyCode::F(1) => app.mode = AppMode::Help,
            KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.open_unified_menu().await;
            }
            KeyCode::Enter => {
                app.send_message();
                app.user_scrolled = false;
                app.scroll_offset = app.chat_max_scroll;
            }
            KeyCode::Esc => {
                if app.focus == Focus::Chat && app.chat_selection.mode != SelectionMode::Normal {
                    app.chat_selection.mode = SelectionMode::Normal;
                    app.chat_selection.anchor_vline = None;
                    app.chat_selection.anchor_col = None;
                } else {
                    app.mode = AppMode::QuitConfirm;
                }
            }
            KeyCode::Tab => {
                app.focus = match app.focus {
                    Focus::Input => Focus::Chat,
                    Focus::Chat => Focus::Tasks,
                    Focus::Tasks => Focus::Input,
                };
                app.chat_selection = ChatSelection::default();
                if app.focus == Focus::Chat {
                    app.chat_selection.vline = app.get_max_vline();
                    app.ensure_vline_visible();
                }
            }
            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_chat();
                app.chat_selection = ChatSelection::default();
            }
            KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(cfg) = app.config.as_mut() {
                    cfg.ui.show_reasoning = !cfg.ui.show_reasoning;
                    app.show_reasoning = cfg.ui.show_reasoning;
                    cfg.save().ok();
                }
            }
            KeyCode::PageUp if app.focus != Focus::Tasks => {
                if !app.user_scrolled {
                    app.scroll_offset = app.chat_max_scroll;
                }
                app.scroll_offset = app.scroll_offset.saturating_sub(10);
                app.user_scrolled = true;
            }
            KeyCode::PageDown if app.focus != Focus::Tasks => {
                app.scroll_offset = app.scroll_offset.saturating_add(10);
                if app.scroll_offset >= app.chat_max_scroll {
                    app.user_scrolled = false;
                }
            }
            KeyCode::Up if app.focus == Focus::Input => {
                if !app.user_scrolled {
                    app.scroll_offset = app.chat_max_scroll;
                }
                app.scroll_offset = app.scroll_offset.saturating_sub(1);
                app.user_scrolled = true;
            }
            KeyCode::Down if app.focus == Focus::Input => {
                if !app.user_scrolled {
                    app.scroll_offset = app.chat_max_scroll;
                }
                app.scroll_offset = app.scroll_offset.saturating_add(1);
                if app.scroll_offset >= app.chat_max_scroll {
                    app.user_scrolled = false;
                }
            }
            // Vim-style selection and navigation when Chat is focused
            KeyCode::Char('k') | KeyCode::Up if app.focus == Focus::Chat => {
                if app.chat_selection.vline > 0 {
                    app.chat_selection.vline -= 1;
                    app.chat_selection.col = app.chat_selection.col.min(app.get_vline_char_count(app.chat_selection.vline));
                    app.ensure_vline_visible();
                }
            }
            KeyCode::Char('j') | KeyCode::Down if app.focus == Focus::Chat => {
                let max_v = app.get_max_vline();
                if app.chat_selection.vline < max_v {
                    app.chat_selection.vline += 1;
                    app.chat_selection.col = app.chat_selection.col.min(app.get_vline_char_count(app.chat_selection.vline));
                    app.ensure_vline_visible();
                }
            }
            KeyCode::Char('h') | KeyCode::Left if app.focus == Focus::Chat => {
                app.chat_selection.col = app.chat_selection.col.saturating_sub(1);
            }
            KeyCode::Char('l') | KeyCode::Right if app.focus == Focus::Chat => {
                let max_c = app.get_vline_char_count(app.chat_selection.vline);
                if app.chat_selection.col < max_c {
                    app.chat_selection.col += 1;
                }
            }
            KeyCode::Char('v') if app.focus == Focus::Chat => {
                if app.chat_selection.mode == SelectionMode::Visual {
                    app.chat_selection.mode = SelectionMode::Normal;
                    app.chat_selection.anchor_vline = None;
                    app.chat_selection.anchor_col = None;
                } else {
                    app.chat_selection.mode = SelectionMode::Visual;
                    app.chat_selection.anchor_vline = Some(app.chat_selection.vline);
                    app.chat_selection.anchor_col = Some(app.chat_selection.col);
                }
            }
            KeyCode::Char('V') if app.focus == Focus::Chat => {
                if app.chat_selection.mode == SelectionMode::VisualLine {
                    app.chat_selection.mode = SelectionMode::Normal;
                    app.chat_selection.anchor_vline = None;
                    app.chat_selection.anchor_col = None;
                } else {
                    app.chat_selection.mode = SelectionMode::VisualLine;
                    app.chat_selection.anchor_vline = Some(app.chat_selection.vline);
                    app.chat_selection.anchor_col = None;
                }
            }
            KeyCode::Char('y') if app.focus == Focus::Chat => {
                if let Some(text) = app.get_selected_text() {
                    if let Some(clipboard) = &mut app.clipboard {
                        if let Err(e) = clipboard.set_text(text) {
                            app.chat_entries.push(ChatEntry::SystemInfo(format!("Clipboard error: {}", e)));
                        } else {
                            app.chat_entries.push(ChatEntry::SystemInfo("Selected text copied to clipboard!".to_string()));
                            app.chat_selection.mode = SelectionMode::Normal;
                            app.chat_selection.anchor_vline = None;
                            app.chat_selection.anchor_col = None;
                        }
                    } else {
                        // Try to re-initialize if it was initially failed
                        match arboard::Clipboard::new() {
                            Ok(mut new_clipboard) => {
                                if let Err(e) = new_clipboard.set_text(text) {
                                    app.chat_entries.push(ChatEntry::SystemInfo(format!("Clipboard error: {}", e)));
                                } else {
                                    app.chat_entries.push(ChatEntry::SystemInfo("Selected text copied to clipboard!".to_string()));
                                    app.chat_selection.mode = SelectionMode::Normal;
                                    app.chat_selection.anchor_vline = None;
                                    app.chat_selection.anchor_col = None;
                                }
                                app.clipboard = Some(new_clipboard);
                            }
                            Err(e) => {
                                app.chat_entries.push(ChatEntry::SystemInfo(format!("Failed to initialize clipboard: {}", e)));
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace if app.focus == Focus::Input => {
                if app.cursor_pos > 0 {
                    let mut chars: Vec<char> = app.input.chars().collect();
                    if app.cursor_pos <= chars.len() {
                        chars.remove(app.cursor_pos - 1);
                        app.input = chars.into_iter().collect();
                        app.cursor_pos -= 1;
                    }
                }
            }
            KeyCode::Delete if app.focus == Focus::Input => {
                let chars: Vec<char> = app.input.chars().collect();
                if app.cursor_pos < chars.len() {
                    let mut new_chars = chars;
                    new_chars.remove(app.cursor_pos);
                    app.input = new_chars.into_iter().collect();
                }
            }
            KeyCode::Left if app.focus == Focus::Input => app.cursor_pos = app.cursor_pos.saturating_sub(1),
            KeyCode::Right if app.focus == Focus::Input => app.cursor_pos = (app.cursor_pos + 1).min(app.input.chars().count()),
            KeyCode::Home if app.focus == Focus::Input => app.cursor_pos = 0,
            KeyCode::End if app.focus == Focus::Input => app.cursor_pos = app.input.chars().count(),
            KeyCode::Char(c) if app.focus == Focus::Input => {
                let mut chars: Vec<char> = app.input.chars().collect();
                if app.cursor_pos <= chars.len() {
                    chars.insert(app.cursor_pos, *c);
                    app.input = chars.into_iter().collect();
                    app.cursor_pos += 1;
                } else {
                    app.input.push(*c);
                    app.cursor_pos = app.input.chars().count();
                }
            }
            _ => {}
        }
    } else if let Event::Mouse(MouseEvent { kind, .. }) = ev {
        if app.focus == Focus::Chat || app.focus == Focus::Input {
            match kind {
                MouseEventKind::ScrollUp => {
                    if !app.user_scrolled {
                        app.scroll_offset = app.chat_max_scroll;
                    }
                    app.scroll_offset = app.scroll_offset.saturating_sub(3);
                    app.user_scrolled = true;
                }
                MouseEventKind::ScrollDown => {
                    app.scroll_offset = app.scroll_offset.saturating_add(3);
                    if app.scroll_offset >= app.chat_max_scroll {
                        app.user_scrolled = false;
                    } else {
                        app.user_scrolled = true;
                    }
                }
                _ => {}
            }
        }
    }
    false
} // handle_main_event

fn handle_quit_confirm(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(KeyEvent { code, .. }) = ev {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => return true,
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => app.mode = AppMode::Main,
            _ => {}
        }
    }
    false
} // handle_quit_confirm

fn render_help_dialog(frame: &mut Frame, area: Rect) {
    let dialog_area = centered_rect(60, 14, area);
    frame.render_widget(ratatui::widgets::Clear, dialog_area);

    let block = ratatui::widgets::Block::default()
        .title(" Seekr Help & Shortcuts ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("    Tab       ", Style::default().fg(Color::Yellow)),
            Span::raw(" Cycle focus: Input ➔ Chat ➔ Tasks"),
        ]),
        Line::from(vec![
            Span::styled("    Up/Down   ", Style::default().fg(Color::Yellow)),
            Span::raw(" Scroll Chat (Input focus) or Select (Chat focus)"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Chat Selection (Focus on Chat panel)",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("    j/k       ", Style::default().fg(Color::Yellow)),
            Span::raw(" Move selection up/down"),
        ]),
        Line::from(vec![
            Span::styled("    y/c       ", Style::default().fg(Color::Yellow)),
            Span::raw(" Yank (Copy) selected message to clipboard"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("    Enter     ", Style::default().fg(Color::Yellow)),
            Span::raw(" Send message"),
        ]),
        Line::from(vec![
            Span::styled("    Ctrl+g    ", Style::default().fg(Color::Yellow)),
            Span::raw(" Open Unified Menu"),
        ]),
        Line::from(vec![
            Span::styled("    Esc       ", Style::default().fg(Color::Yellow)),
            Span::raw(" Quit Seekr"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled(
                "any key",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to close"),
        ]),
    ];

    frame.render_widget(Paragraph::new(text).block(block), dialog_area);
} // render_help_dialog
