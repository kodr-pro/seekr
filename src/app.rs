use crate::agent::loop_mod::AgentLoop;
use crate::agent::{AgentCommand, AgentEvent};
use crate::api::client::ApiClient;
use crate::app_state::{AgentState, SessionState, UiState};
use crate::config::AppConfig;
use crate::errors::ApiError;
use crate::event_handler::handle_event;
use crate::tools::task::Task;
use crate::ui::render::render;
use anyhow::Result;
use crossterm::event::EventStream;
use ratatui::DefaultTerminal;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

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

#[derive(Debug, Clone)]
pub enum InputMode {
    Normal,
    ShellStdin {
        context: String,
        input_tx: tokio::sync::mpsc::UnboundedSender<String>,
    },
    EditingProviderKey {
        provider_idx: usize,
    },
    EditingProviderName {
        provider_idx: usize,
    },
    EditingProviderUrl {
        provider_idx: usize,
    },
    EditingProviderModel {
        provider_idx: usize,
    },
}

impl PartialEq for InputMode {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (InputMode::Normal, InputMode::Normal)
                | (InputMode::ShellStdin { .. }, InputMode::ShellStdin { .. })
                | (
                    InputMode::EditingProviderKey { .. },
                    InputMode::EditingProviderKey { .. }
                )
                | (
                    InputMode::EditingProviderName { .. },
                    InputMode::EditingProviderName { .. }
                )
                | (
                    InputMode::EditingProviderUrl { .. },
                    InputMode::EditingProviderUrl { .. }
                )
                | (
                    InputMode::EditingProviderModel { .. },
                    InputMode::EditingProviderModel { .. }
                )
        )
    }
} // eq

#[derive(Debug, Clone)]
pub enum ChatEntry {
    UserMessage(String),
    AssistantContent(String),
    AssistantStreaming(String),
    Reasoning(String),
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        result: String,
    },
    Error(String),
    SystemInfo(String),
    ToolApproval {
        name: String,
        arguments: String,
    },
    CliInputPrompt(String),
    ContextSummary {
        id: String,
        summary: String,
        is_pending: bool,
    },
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

#[derive(Debug)]
pub enum BgEvent {
    ModelsFetched(Result<Vec<String>, ApiError>),
    UpdateAvailable(String),
}

#[derive(Debug, Clone)]
pub struct MenuState {
    pub active_tab: MenuTab,
    pub selection_idx: usize,
    pub scroll_offset: usize,
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
    pub input: String,
    pub cursor_pos: usize,

    pub ui: UiState,
    pub agent: AgentState,
    pub session: SessionState,

    pub input_mode: InputMode,

    pub tasks: Vec<Task>,

    pub menu_state: MenuState,

    pub manager: Option<std::sync::Arc<crate::manager::SeekrManager>>,
    pub setup_state: SetupState,

    pub clipboard: Option<arboard::Clipboard>,

    pub bg_tx: tokio::sync::mpsc::UnboundedSender<BgEvent>,
    pub bg_rx: tokio::sync::mpsc::UnboundedReceiver<BgEvent>,

    pub entry_vlines: Vec<Vec<VisualLine>>,
    pub new_version_available: Option<String>,
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
            input: String::new(),
            cursor_pos: 0,
            ui: UiState::default(),
            agent: AgentState::default(),
            session: SessionState::default(),
            input_mode: InputMode::Normal,
            tasks: Vec::new(),
            menu_state: MenuState::default(),
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
            clipboard: arboard::Clipboard::new().ok(),
            bg_tx,
            bg_rx,
            entry_vlines: Vec::new(),
            new_version_available: None,
        }
    }

    pub fn new_main(config: AppConfig) -> Self {
        let show_reasoning = config.ui.show_reasoning;
        let manager = std::sync::Arc::new(crate::manager::SeekrManager::new(config.clone()));
        let mut app = Self {
            mode: AppMode::Main,
            config: Some(config.clone()),
            manager: Some(manager),
            ..Self::new_setup()
        };
        app.ui.show_reasoning = show_reasoning;
        app.chat_entries.push(ChatEntry::SystemInfo(
            "Welcome to Seekr! Type a message to start.".to_string(),
        ));

        if config.agent.show_shell_warnings {
            let warning = "SECURITY WARNING: The shell tool is enabled. Seekr can execute terminal commands. A basic blocklist is active, but you should review commands before execution or run in an isolated environment.";
            app.chat_entries.push(ChatEntry::Error(warning.to_string()));
        }

        app.ui.needs_recompute_vlines = true;
        app
    }

    pub fn start_agent(&mut self) {
        let config = match self.config.as_ref() {
            Some(c) => c,
            None => return,
        };

        if let Some(ref tx) = self.agent.cmd_tx {
            tx.send(AgentCommand::Shutdown).ok();
        }

        let (evt_tx, evt_rx) = mpsc::unbounded_channel();
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

        let agent_res = if let Some(sid) = &self.session.session_id {
            let registry = match self.manager.as_ref() {
                Some(m) => m.tool_registry(),
                None => {
                    self.chat_entries
                        .push(ChatEntry::Error("Manager not initialized".to_string()));
                    return;
                }
            };
            AgentLoop::resume(
                config.clone(),
                sid,
                evt_tx,
                cmd_rx,
                cmd_tx.clone(),
                registry,
            )
        } else {
            let registry = match self.manager.as_ref() {
                Some(m) => m.tool_registry(),
                None => {
                    self.chat_entries
                        .push(ChatEntry::Error("Manager not initialized".to_string()));
                    return;
                }
            };
            Ok(AgentLoop::new(
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
                self.agent.cmd_tx = Some(cmd_tx.clone());
                self.agent.event_rx = Some(evt_rx);
                self.agent.provider_connected =
                    vec![false; self.config.as_ref().unwrap().providers.len()];
                cmd_tx.send(AgentCommand::CheckConnection).ok();
            }
            Err(e) => {
                self.chat_entries
                    .push(ChatEntry::Error(format!("Failed to start agent: {}", e)));
            }
        }
    } // start_agent

    pub fn resume_session(&mut self, session_id: String) {
        self.session.session_id = Some(session_id.clone());
        match crate::session::Session::load(&session_id) {
            Ok(session) => {
                self.chat_entries.clear();
                self.ui.needs_recompute_vlines = true;
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
                                    summary: content
                                        .replace("--- PAST CONTEXT SUMMARY ---\n", "")
                                        .replace("\n----------------------------", ""),
                                    is_pending: false,
                                });
                            } else if content.contains("[Summarizing context segment") {
                                let id = content
                                    .split_whitespace()
                                    .last()
                                    .unwrap_or("")
                                    .trim_end_matches("...]")
                                    .to_string();
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
            self.ui.needs_recompute_vlines = true;
            return;
        }

        self.chat_entries.push(ChatEntry::UserMessage(msg.clone()));
        self.update_vlines_for_entry(self.chat_entries.len() - 1, self.ui.last_chat_width);
        self.agent.is_streaming = true;
        self.agent.streaming_content.clear();
        self.agent.streaming_reasoning.clear();
        self.ui.user_scrolled = false;

        if let Some(ref tx) = self.agent.cmd_tx {
            tx.send(AgentCommand::UserMessage(msg)).ok();
        }

        self.ui.scroll_offset = self.ui.chat_max_scroll;
    } // send_message

    pub fn poll_agent_events(&mut self) {
        let events: Vec<AgentEvent> = {
            let rx = match self.agent.event_rx.as_mut() {
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
                AgentEvent::ProviderStatus { index, connected } => {
                    if self.agent.provider_connected.len() <= index {
                        self.agent.provider_connected.resize(index + 1, false);
                    }
                    self.agent.provider_connected[index] = connected;

                    // Update main connected light if this is the active provider
                    if self
                        .config
                        .as_ref()
                        .is_some_and(|c| c.active_provider == index)
                    {
                        self.agent.connected = connected;
                    }
                }
                AgentEvent::ContentDelta(text) => {
                    self.agent.connected = true; // Also treat first delta as connected
                    self.agent.streaming_content.push_str(&text);
                    self.update_streaming_entry();
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
                AgentEvent::ReasoningDelta(text) => {
                    self.agent.streaming_reasoning.push_str(&text);
                    if self.ui.show_reasoning {
                        self.update_reasoning_entry();
                    }
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
                AgentEvent::ToolCallStart { name, arguments } => {
                    self.finalize_streaming();
                    self.chat_entries
                        .push(ChatEntry::ToolCall { name, arguments });
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
                AgentEvent::ToolCallResult { name, result } => {
                    self.chat_entries
                        .push(ChatEntry::ToolResult { name, result });
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
                AgentEvent::Activity(entry) => {
                    self.agent.activities.push(entry.clone());
                    if let Some(tid) = entry.thread_id {
                        if entry.status == crate::tools::ActivityStatus::Starting {
                            self.agent.live_activities.push(entry);
                        } else {
                            self.agent
                                .live_activities
                                .retain(|a| a.thread_id != Some(tid));
                        }
                    }
                }
                AgentEvent::TokenUsage { total_tokens, .. } => {
                    self.agent.total_tokens = total_tokens;
                }
                AgentEvent::IterationUpdate(n) => {
                    self.agent.iteration = n;
                }
                AgentEvent::TurnComplete => {
                    self.finalize_streaming();
                    self.agent.is_streaming = false;
                    self.agent.iteration = 0;
                    if self.mode == AppMode::AwaitingContinue {
                        self.mode = AppMode::Main;
                    }
                }
                AgentEvent::MaxIterationsReached => {
                    self.finalize_streaming();
                    self.agent.is_streaming = false;
                    self.mode = AppMode::AwaitingContinue;
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
                AgentEvent::Error(msg) => {
                    self.finalize_streaming();
                    self.agent.is_streaming = false;
                    self.chat_entries.push(ChatEntry::Error(msg.to_string()));
                }
                AgentEvent::ToolApprovalRequest {
                    name, arguments, ..
                } => {
                    self.agent.awaiting_approval = true;
                    self.chat_entries
                        .push(ChatEntry::ToolApproval { name, arguments });
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
                AgentEvent::ShellInputNeeded { context, input_tx } => {
                    self.input_mode = InputMode::ShellStdin { context, input_tx };
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
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
                    let start_idx = self
                        .chat_entries
                        .iter()
                        .position(|e| !matches!(e, ChatEntry::SystemInfo(_)))
                        .unwrap_or(0);
                    let mut removed = 0;
                    let mut i = start_idx;
                    while removed < count && i < self.chat_entries.len() {
                        if matches!(
                            self.chat_entries[i],
                            ChatEntry::UserMessage(_)
                                | ChatEntry::AssistantContent(_)
                                | ChatEntry::ToolCall { .. }
                                | ChatEntry::ToolResult { .. }
                                | ChatEntry::Reasoning(_)
                        ) {
                            self.chat_entries.remove(i);
                            if i < self.entry_vlines.len() {
                                self.entry_vlines.remove(i);
                            }
                            removed += 1;
                        } else {
                            i += 1;
                        }
                    }
                    self.sync_visual_lines();
                }
                AgentEvent::ContextSummaryReady { id, summary } => {
                    if let Some(pos) = self.chat_entries.iter().rposition(|e| match e {
                        ChatEntry::ContextSummary { id: s_id, .. } => s_id == &id,
                        _ => false,
                    }) {
                        if let ChatEntry::ContextSummary {
                            summary: ref mut s,
                            is_pending: ref mut p,
                            ..
                        } = self.chat_entries[pos]
                        {
                            *s = summary;
                            *p = false;
                        }
                    } else {
                        self.chat_entries.push(ChatEntry::ContextSummary {
                            id,
                            summary,
                            is_pending: false,
                        });
                        self.update_vlines_for_entry(
                            self.chat_entries.len() - 1,
                            self.ui.last_chat_width,
                        );
                    }
                    self.sync_visual_lines();
                    if !self.ui.user_scrolled {
                        self.ui.scroll_offset = self.ui.chat_max_scroll;
                    }
                }
            }
        }
    } // poll_agent_events

    fn update_streaming_entry(&mut self) {
        let last_idx = self.chat_entries.len().saturating_sub(1);
        if let Some(ChatEntry::AssistantStreaming(content)) = self.chat_entries.last_mut() {
            *content = self.agent.streaming_content.clone();
            self.update_vlines_for_entry(last_idx, self.ui.last_chat_width);
            return;
        }
        self.chat_entries.push(ChatEntry::AssistantStreaming(
            self.agent.streaming_content.clone(),
        ));
        self.update_vlines_for_entry(self.chat_entries.len() - 1, self.ui.last_chat_width);
    } // end update_streaming_entry

    fn update_reasoning_entry(&mut self) {
        let len = self.chat_entries.len();
        for i in (0..len).rev() {
            if let ChatEntry::Reasoning(ref mut text) = self.chat_entries[i] {
                *text = self.agent.streaming_reasoning.clone();
                self.update_vlines_for_entry(i, self.ui.last_chat_width);
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
            ChatEntry::Reasoning(self.agent.streaming_reasoning.clone()),
        );
        while self.entry_vlines.len() < insert_pos {
            self.entry_vlines.push(Vec::new());
        }
        self.entry_vlines.insert(insert_pos, Vec::new());
        self.update_vlines_for_entry(insert_pos, self.ui.last_chat_width);
    } // end update_reasoning_entry

    fn finalize_streaming(&mut self) {
        if !self.agent.streaming_content.is_empty() {
            if let Some(pos) = self
                .chat_entries
                .iter()
                .rposition(|e| matches!(e, ChatEntry::AssistantStreaming(_)))
            {
                self.chat_entries[pos] =
                    ChatEntry::AssistantContent(self.agent.streaming_content.clone());
                self.update_vlines_for_entry(pos, self.ui.last_chat_width);
            } else {
                self.chat_entries.push(ChatEntry::AssistantContent(
                    self.agent.streaming_content.clone(),
                ));
                self.update_vlines_for_entry(self.chat_entries.len() - 1, self.ui.last_chat_width);
            }
            self.agent.streaming_content.clear();
        }
        self.agent.streaming_reasoning.clear();
    } // finalize_streaming

    pub fn handle_approval(&mut self, approved: bool, always: bool) {
        self.agent.awaiting_approval = false;
        if let Some(pos) = self
            .chat_entries
            .iter()
            .rposition(|e| matches!(e, ChatEntry::ToolApproval { .. }))
        {
            self.chat_entries.remove(pos);
        }

        if let Some(ref tx) = self.agent.cmd_tx {
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
        self.ui.scroll_offset = 0;
        self.ui.needs_recompute_vlines = true;
    } // clear_chat

    pub async fn load_sessions(&mut self) {
        if let Some(ref mgr) = self.manager {
            match mgr.load_sessions().await {
                Ok(_) => {
                    self.session.sessions = mgr.list_sessions().await;
                    self.session.session_list_error = None;
                }
                Err(e) => {
                    self.session.session_list_error = Some(format!("Failed to load sessions: {e}"));
                    self.session.sessions.clear();
                }
            }
        } else {
            match crate::session::Session::list_all() {
                Ok(s) => {
                    self.session.sessions = s;
                    self.session.session_list_error = None;
                }
                Err(e) => {
                    self.session.session_list_error = Some(format!("Failed to load sessions: {e}"));
                    self.session.sessions.clear();
                }
            }
        }
    } // load_sessions

    pub fn poll_bg_events(&mut self) {
        while let Ok(event) = self.bg_rx.try_recv() {
            match event {
                BgEvent::ModelsFetched(Ok(models)) => {
                    self.session.available_models = models;
                }
                BgEvent::ModelsFetched(Err(e)) => {
                    self.chat_entries
                        .push(ChatEntry::Error(format!("Failed to fetch models: {}", e)));
                }
                BgEvent::UpdateAvailable(version) => {
                    self.new_version_available = Some(version);
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
            let res = client.list_models().await;
            let _ = tx.send(BgEvent::ModelsFetched(res));
        });
    } // fetch_available_models

    pub fn check_for_updates(&self) {
        let tx = self.bg_tx.clone();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .user_agent("seekr-tui")
                .build()
                .unwrap_or_default();

            let res = client
                .get("https://api.github.com/repos/kodr-pro/seekr/releases/latest")
                .send()
                .await;

            if let Ok(response) = res
                && let Ok(json) = response.json::<serde_json::Value>().await
                && let Some(tag) = json["tag_name"].as_str()
            {
                let current = env!("CARGO_PKG_VERSION");
                let version = tag.trim_start_matches('v');

                match (
                    semver::Version::parse(current),
                    semver::Version::parse(version),
                ) {
                    (Ok(current_v), Ok(latest_v)) => {
                        if latest_v > current_v {
                            let _ = tx.send(BgEvent::UpdateAvailable(version.to_string()));
                        }
                    }
                    _ => {
                        if version != current {
                            // Fallback to basic string comparison if semver fails
                            let _ = tx.send(BgEvent::UpdateAvailable(version.to_string()));
                        }
                    }
                }
            }
        });
    } // check_for_updates

    pub async fn open_unified_menu(&mut self) {
        self.load_sessions().await;
        if self.session.available_models.is_empty() {
            self.fetch_available_models();
        }
        self.mode = AppMode::UnifiedMenu;
        self.menu_state = MenuState::default();
    } // open_unified_menu

    pub async fn delete_session_at(&mut self, idx: usize) {
        let session = match self.session.sessions.get(idx) {
            Some(s) => s,
            None => return,
        };
        let id = session.id.clone();

        if let Some(ref mgr) = self.manager {
            if let Err(e) = mgr.delete_session(&id).await {
                self.session.session_list_error = Some(format!("Failed to delete session: {e}"));
                return;
            }
            self.session.sessions = mgr.list_sessions().await;
        } else {
            if let Ok(dir) = crate::session::Session::sessions_dir() {
                let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
            }
            self.session.sessions.retain(|s| s.id != id);
        }
    } // delete_session_at
    pub fn rebuild_vlines_cache(&mut self, width: u16) {
        self.entry_vlines.clear();
        for i in 0..self.chat_entries.len() {
            let lines = self.calculate_lines_for_entry(i, width);
            self.entry_vlines.push(lines);
        }
        self.ui.last_chat_width = width;
        self.sync_visual_lines();
    }

    pub fn update_vlines_for_entry(&mut self, idx: usize, width: u16) {
        if idx >= self.chat_entries.len() {
            return;
        }
        let lines = self.calculate_lines_for_entry(idx, width);
        while self.entry_vlines.len() <= idx {
            self.entry_vlines.push(Vec::new());
        }
        self.entry_vlines[idx] = lines;
        self.sync_visual_lines();
    }

    pub fn sync_visual_lines(&mut self) {
        self.visual_lines = self.entry_vlines.iter().flatten().cloned().collect();
        self.ui.needs_recompute_vlines = false;
    }

    fn calculate_lines_for_entry(&self, idx: usize, width: u16) -> Vec<VisualLine> {
        let mut visual_lines = Vec::new();
        if width == 0 {
            return visual_lines;
        }
        let entry = match self.chat_entries.get(idx) {
            Some(e) => e,
            None => return visual_lines,
        };

        let (header, content) = match entry {
            ChatEntry::UserMessage(msg) => (Some("[YOU]"), msg.as_str()),
            ChatEntry::AssistantContent(text) | ChatEntry::AssistantStreaming(text) => (
                Some("[SEEKR]"),
                if text.is_empty() && matches!(entry, ChatEntry::AssistantStreaming(_)) {
                    "..."
                } else {
                    text.as_str()
                },
            ),
            ChatEntry::Reasoning(text) => (Some("[THINKING]"), text.as_str()),
            ChatEntry::ToolCall { name, arguments } => {
                let args_short = if arguments.len() > 64 {
                    format!("{}...", &arguments[..64])
                } else {
                    arguments.clone()
                };
                (None, &*format!("➞ Tool Call: {} ({})", name, args_short))
            }
            ChatEntry::ToolResult { name, result } => {
                let max_len = 2000;
                let display_result = if result.len() > max_len {
                    format!("{}... (truncated)", &result[..max_len])
                } else {
                    result.clone()
                };
                (
                    None,
                    &*format!("✓ Tool Result: {}\n{}", name, display_result),
                )
            }
            ChatEntry::Error(msg) => (Some("[ERROR]"), msg.as_str()),
            ChatEntry::SystemInfo(msg) => (None, &*format!("[INFO] {}", msg)),
            ChatEntry::ToolApproval { name, arguments } => (
                Some("[APPROVAL REQUIRED]"),
                &*format!(
                    "Agent wants to execute: {}({})\n  [Y]es / [N]o / [A]lways",
                    name, arguments
                ),
            ),
            ChatEntry::CliInputPrompt(prompt) => (Some("[INPUT REQUIRED]"), prompt.as_str()),
            ChatEntry::ContextSummary {
                summary,
                is_pending,
                ..
            } => {
                if *is_pending {
                    (
                        Some("[CONTEXT WINDOW]"),
                        "Summarizing past conversation to free up context space...",
                    )
                } else {
                    (Some("[CONTEXT SUMMARY]"), summary.as_str())
                }
            }
        };

        if idx > 0 {
            visual_lines.push(VisualLine {
                entry_idx: idx,
                text: String::new(),
                is_header: false,
                line_type: LineType::Normal,
                language: None,
            });
        }

        if let Some(h) = header {
            visual_lines.push(VisualLine {
                entry_idx: idx,
                text: h.to_string(),
                is_header: true,
                line_type: LineType::Normal,
                language: None,
            });
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
                    let after_backticks = &trimmed[3..].trim();
                    if !after_backticks.is_empty() {
                        let lang = after_backticks.split_whitespace().next().unwrap_or("");
                        if !lang.is_empty() {
                            current_language = Some(lang.to_string());
                        }
                    }
                    language = current_language.clone();
                } else {
                    line_type = LineType::CodeBlockEnd;
                    current_language = None;
                }
                in_code_block = !in_code_block;
            } else if in_code_block {
                line_type = LineType::CodeBlock;
                language = current_language.clone();
            }

            let line_to_wrap = if line_type == LineType::CodeBlockStart {
                format!("{} [COPY]", line)
            } else {
                line.to_string()
            };

            if line.is_empty() {
                visual_lines.push(VisualLine {
                    entry_idx: idx,
                    text: String::new(),
                    is_header: false,
                    line_type,
                    language,
                });
                continue;
            }

            let words = line_to_wrap.split_inclusive(' ');
            let mut current_line = String::new();
            for word in words {
                if current_line.chars().count() + word.chars().count() > width as usize {
                    if !current_line.is_empty() {
                        visual_lines.push(VisualLine {
                            entry_idx: idx,
                            text: current_line.clone(),
                            is_header: false,
                            line_type,
                            language: language.clone(),
                        });
                        current_line.clear();
                    }
                    let mut remaining_word = word;
                    while remaining_word.chars().count() > width as usize {
                        let split_idx = remaining_word
                            .char_indices()
                            .map(|(i, _)| i)
                            .nth(width as usize)
                            .unwrap_or(remaining_word.len());
                        let (head, tail) = remaining_word.split_at(split_idx);
                        visual_lines.push(VisualLine {
                            entry_idx: idx,
                            text: head.to_string(),
                            is_header: false,
                            line_type,
                            language: language.clone(),
                        });
                        remaining_word = tail;
                    }
                    current_line.push_str(remaining_word);
                } else {
                    current_line.push_str(word);
                }
            }
            if !current_line.is_empty() {
                visual_lines.push(VisualLine {
                    entry_idx: idx,
                    text: current_line,
                    is_header: false,
                    line_type,
                    language,
                });
            }
        }
        visual_lines
    }

    pub fn get_max_vline(&self) -> usize {
        self.visual_lines.len().saturating_sub(1)
    }

    pub fn get_vline_char_count(&self, vline_idx: usize) -> usize {
        self.visual_lines
            .get(vline_idx)
            .map(|l| l.text.chars().count())
            .unwrap_or(0)
    }
} // impl App

pub async fn run_app(mut app: App) -> Result<()> {
    let mut terminal = ratatui::init();

    app.check_for_updates();
    if app.mode == AppMode::Main {
        app.start_agent();
    }

    let result = event_loop(&mut terminal, &mut app).await;

    ratatui::restore();

    if let Some(ref tx) = app.agent.cmd_tx {
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
                if let Some(Ok(ev)) = maybe_ev
                    && handle_event(app, &ev).await?
                {
                    return Ok(());
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
