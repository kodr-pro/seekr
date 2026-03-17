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
}

impl PartialEq for InputMode {
    fn eq(&self, other: &Self) -> bool {
        matches!((self, other), (InputMode::Normal, InputMode::Normal) | (InputMode::ShellStdin { .. }, InputMode::ShellStdin { .. }))
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
}

impl App {
    pub fn new_setup() -> Self {
        Self {
            mode: AppMode::Setup,
            focus: Focus::Chat,
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
            crate::agent::loop_mod::AgentLoop::resume(config.clone(), sid, evt_tx, cmd_rx, registry)
        } else {
            let registry = self.manager.as_ref().unwrap().tool_registry();
            Ok(crate::agent::loop_mod::AgentLoop::new(config.clone(), evt_tx, cmd_rx, registry))
        };

        match agent_res {
            Ok(agent) => {
                tokio::spawn(agent.run());
                self.agent_cmd_tx = Some(cmd_tx);
                self.agent_event_rx = Some(evt_rx);
                self.connected = true;
            }
            Err(e) => {
                self.chat_entries.push(ChatEntry::Error(format!("Failed to start agent: {}", e)));
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
                        ("user", Some(content)) => self.chat_entries.push(ChatEntry::UserMessage(content)),
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
                        _ => {}
                    }
                }
                self.tasks = session.task_manager.tasks();
            }
            Err(e) => {
                self.chat_entries.push(ChatEntry::Error(format!("Failed to load session: {}", e)));
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
                    self.chat_entries.push(ChatEntry::ToolCall { name, arguments });
                    if !self.user_scrolled {
                        self.scroll_offset = self.chat_max_scroll;
                    }
                }
                AgentEvent::ToolCallResult { name, result } => {
                    self.chat_entries.push(ChatEntry::ToolResult { name, result });
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
                AgentEvent::ToolApprovalRequest { name, arguments, .. } => {
                    self.awaiting_approval = true;
                    self.chat_entries.push(ChatEntry::ToolApproval { name, arguments });
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
            }
        }
    } // poll_agent_events

    fn update_streaming_entry(&mut self) {
        if let Some(ChatEntry::AssistantStreaming(ref mut content)) = self.chat_entries.last_mut() {
            *content = self.streaming_content.clone();
            return;
        }
        self.chat_entries.push(ChatEntry::AssistantStreaming(self.streaming_content.clone()));
    } // end update_streaming_entry

    fn update_reasoning_entry(&mut self) {
        let len = self.chat_entries.len();
        for i in (0..len).rev() {
            if let ChatEntry::Reasoning(ref mut text) = self.chat_entries[i] {
                *text = self.streaming_reasoning.clone();
                return;
            }
            if !matches!(&self.chat_entries[i], ChatEntry::AssistantStreaming(_) | ChatEntry::Reasoning(_)) {
                break;
            }
        }
        
        let insert_pos = if matches!(self.chat_entries.last(), Some(ChatEntry::AssistantStreaming(_))) {
            self.chat_entries.len() - 1
        } else {
            self.chat_entries.len()
        };
        self.chat_entries.insert(insert_pos, ChatEntry::Reasoning(self.streaming_reasoning.clone()));
    } // end update_reasoning_entry

    fn finalize_streaming(&mut self) {
        if !self.streaming_content.is_empty() {
            if let Some(pos) = self.chat_entries.iter().rposition(|e| matches!(e, ChatEntry::AssistantStreaming(_))) {
                self.chat_entries[pos] = ChatEntry::AssistantContent(self.streaming_content.clone());
            } else {
                self.chat_entries.push(ChatEntry::AssistantContent(self.streaming_content.clone()));
            }
            self.streaming_content.clear();
        }
        self.streaming_reasoning.clear();
    } // finalize_streaming

    pub fn handle_approval(&mut self, approved: bool, always: bool) {
        self.awaiting_approval = false;
        if let Some(pos) = self.chat_entries.iter().rposition(|e| matches!(e, ChatEntry::ToolApproval { .. })) {
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
        self.chat_entries.push(ChatEntry::SystemInfo("Chat cleared. Type a message to continue.".to_string()));
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

    pub async fn fetch_available_models(&mut self) {
        if self.config.is_none() {
            return;
        }
        let client = ApiClient::new(self.config.as_ref().unwrap());
        match client.list_models().await {
            Ok(models) => {
                self.available_models = models;
            }
            Err(e) => {
                self.chat_entries.push(ChatEntry::Error(format!("Failed to fetch models: {}", e)));
            }
        }
    } // fetch_available_models

    pub async fn open_unified_menu(&mut self) {
        self.load_sessions().await;
        if self.available_models.is_empty() {
            self.fetch_available_models().await;
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

        if app.mode == AppMode::Main || app.mode == AppMode::AwaitingContinue {
            app.poll_agent_events();
        }

        if event::poll(Duration::from_millis(16))? {
            let ev = event::read()?;
            match app.mode {
                AppMode::Setup => if handle_setup_event(app, &ev).await? { return Ok(()); },
                AppMode::Main | AppMode::AwaitingContinue => if handle_main_event(app, &ev).await { return Ok(()); },
                AppMode::QuitConfirm => if handle_quit_confirm(app, &ev) { return Ok(()); },
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
                            app.chat_entries.push(ChatEntry::SystemInfo(format!("Switched to model: {}", model_clone)));
                            app.start_agent();
                        }
                    }
                }
                MenuTab::Providers => {
                    if let Some(cfg) = app.config.as_mut() {
                        cfg.active_provider = app.menu_state.selection_idx;
                        cfg.save().ok();
                        app.mode = AppMode::Main;
                        app.chat_entries.push(ChatEntry::SystemInfo(format!("Switched to provider: {}", cfg.current_provider().name)));
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
                                    _ => 15,
                                };
                            }
                            2 => { cfg.agent.auto_approve_tools = !cfg.agent.auto_approve_tools; }
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
        _ => {}
    }
} // handle_unified_menu_event


fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    match app.mode {
        AppMode::Setup => ui::setup::render_setup(frame, area, &app.setup_state),
        AppMode::Main | AppMode::QuitConfirm | AppMode::AwaitingContinue | AppMode::Help | AppMode::UnifiedMenu => {
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

    app.chat_max_scroll = ui::chat::render_chat(
        frame,
        layout.chat_panel,
        &app.chat_entries,
        app.scroll_offset,
        app.focus == Focus::Chat,
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
        InputMode::ShellStdin { context, .. } => (if context.is_empty() { None } else { Some(context.as_str()) }, Some("Shell input required")),
        InputMode::Normal => (None, None),
    };

    ui::input::render_input(
        frame,
        layout.input_bar,
        &app.input,
        app.cursor_pos,
        !app.awaiting_approval,
        input_prompt,
        shell_context,
    );

    let model = app.config.as_ref().map(|c| c.current_provider().model.as_str()).unwrap_or("unknown");
    let provider = app.config.as_ref().map(|c| c.current_provider().name.as_str()).unwrap_or("unknown");
    let max_iter = app.config.as_ref().map(|c| c.agent.max_iterations).unwrap_or(15);
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
    let model = app.config.as_ref().map(|c| c.current_provider().model.as_str()).unwrap_or("unknown");
    let status = if app.is_streaming { "Working" } else { "Ready" };

    ui::title::render_title(frame, area, &ui::title::TitleInfo {
        version: "0.1.0",
        session_id: app.session_id.as_deref(),
        connected: app.connected,
        model,
        status,
    });
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
        Line::from(Span::styled("    Are you sure you want to quit?", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![
            Span::raw("    Press "),
            Span::styled("[Y]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to Yes, "),
            Span::styled("[N]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
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
        Line::from(Span::styled("  The agent has used all available iterations.", Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled("[C]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" to Continue   "),
            Span::styled("[A]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
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
            0 => if key.code == KeyCode::Enter { app.setup_state.current_step = 1; },
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
                KeyCode::Backspace => { app.setup_state.api_key_input.pop(); },
                KeyCode::Char(c) => { app.setup_state.api_key_input.push(c); app.setup_state.error_message = None; }
                _ => {}
            },
            2 => match key.code {
                KeyCode::Up => app.setup_state.model_selection = app.setup_state.model_selection.saturating_sub(1),
                KeyCode::Down => app.setup_state.model_selection = (app.setup_state.model_selection + 1).min(1),
                KeyCode::Enter => app.setup_state.current_step = 3,
                KeyCode::Esc => app.setup_state.current_step = 1,
                _ => {}
            },
            3 => match key.code {
                KeyCode::Up => app.setup_state.auto_approve_selection = app.setup_state.auto_approve_selection.saturating_sub(1),
                KeyCode::Down => app.setup_state.auto_approve_selection = (app.setup_state.auto_approve_selection + 1).min(1),
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
                    let valid = ApiClient::validate_key(&key, "https://api.deepseek.com", "deepseek-chat").await;
                    app.setup_state.validating = false;

                    match valid {
                        Ok(true) => {
                            let model = if app.setup_state.model_selection == 0 { "deepseek-chat" } else { "deepseek-reasoner" };
                            let auto_approve = app.setup_state.auto_approve_selection == 1;
                            let working_dir = if app.setup_state.working_dir_input.is_empty() { ".".to_string() } else { app.setup_state.working_dir_input.clone() };

                            let config = AppConfig {
                                providers: vec![crate::config::ProviderConfig {
                                    name: "DeepSeek".to_string(),
                                    key: app.setup_state.api_key_input.clone(),
                                    base_url: "https://api.deepseek.com".to_string(),
                                    model: model.to_string(),
                                }],
                                active_provider: 0,
                                agent: crate::config::AgentConfig { max_iterations: 15, auto_approve_tools: auto_approve, working_directory: working_dir },
                                ui: crate::config::UiConfig { theme: "dark".to_string(), show_reasoning: true },
                            };

                            if let Err(e) = config.save() {
                                app.setup_state.error_message = Some(format!("Failed to save config: {e}"));
                            } else {
                                app.config = Some(config);
                                app.setup_state.current_step = 6;
                            }
                        }
                        Ok(false) => { app.setup_state.error_message = Some("Invalid API key.".to_string()); }
                        Err(e) => { app.setup_state.error_message = Some(format!("Connection error: {e}")); }
                    }
                }
                KeyCode::Esc => app.setup_state.current_step = 3,
                KeyCode::Backspace => { app.setup_state.working_dir_input.pop(); }
                KeyCode::Char(c) => { app.setup_state.working_dir_input.push(c); }
                _ => {}
            },
            5 => if key.code == KeyCode::Enter { app.setup_state.current_step = 1; app.setup_state.error_message = None; },
            6 => if key.code == KeyCode::Enter {
                app.mode = AppMode::Main;
                app.show_reasoning = true;
                app.chat_entries.push(ChatEntry::SystemInfo("Welcome to Seekr!".to_string()));
                app.start_agent();
            }
            _ => {}
        }
    }
    Ok(false)
} // handle_setup_event

pub async fn handle_main_event(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(KeyEvent { code, modifiers, .. }) = ev {
        if *code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) { return true; }

        if app.mode == AppMode::Help { app.mode = AppMode::Main; return false; }

        if app.mode == AppMode::AwaitingContinue {
            match code {
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    app.mode = AppMode::Main;
                    app.is_streaming = true;
                    app.user_scrolled = false;
                    if let Some(ref tx) = app.agent_cmd_tx { tx.send(AgentCommand::Continue).ok(); }
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    app.mode = AppMode::Main;
                    if let Some(ref tx) = app.agent_cmd_tx { tx.send(AgentCommand::AnswerNow).ok(); }
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
            KeyCode::Enter => { app.send_message(); app.user_scrolled = false; app.scroll_offset = app.chat_max_scroll; }
            KeyCode::Esc => app.mode = AppMode::QuitConfirm,
            KeyCode::Tab => app.focus = if app.focus == Focus::Chat { Focus::Tasks } else { Focus::Chat },
            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => app.clear_chat(),
            KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(cfg) = app.config.as_mut() {
                    cfg.ui.show_reasoning = !cfg.ui.show_reasoning;
                    app.show_reasoning = cfg.ui.show_reasoning;
                    cfg.save().ok();
                }
            }
            KeyCode::Char('m') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.open_unified_menu().await;
            }
            KeyCode::PageUp if app.focus == Focus::Chat => {
                if !app.user_scrolled { app.scroll_offset = app.chat_max_scroll; }
                app.scroll_offset = app.scroll_offset.saturating_sub(10);
                app.user_scrolled = true;
            }
            KeyCode::PageDown if app.focus == Focus::Chat => {
                app.scroll_offset = app.scroll_offset.saturating_add(10);
                if app.scroll_offset >= app.chat_max_scroll { app.user_scrolled = false; }
            }
            KeyCode::Up if app.focus == Focus::Chat => {
                if !app.user_scrolled { app.scroll_offset = app.chat_max_scroll; }
                app.scroll_offset = app.scroll_offset.saturating_sub(1);
                app.user_scrolled = true;
            }
            KeyCode::Down if app.focus == Focus::Chat => {
                app.scroll_offset = app.scroll_offset.saturating_add(1);
                if app.scroll_offset >= app.chat_max_scroll { app.user_scrolled = false; }
            }
            KeyCode::Backspace => {
                if app.cursor_pos > 0 {
                    let mut chars: Vec<char> = app.input.chars().collect();
                    if app.cursor_pos <= chars.len() {
                        chars.remove(app.cursor_pos - 1);
                        app.input = chars.into_iter().collect();
                        app.cursor_pos -= 1;
                    }
                }
            }
            KeyCode::Delete => {
                let chars: Vec<char> = app.input.chars().collect();
                if app.cursor_pos < chars.len() {
                    let mut new_chars = chars;
                    new_chars.remove(app.cursor_pos);
                    app.input = new_chars.into_iter().collect();
                }
            }
            KeyCode::Left => app.cursor_pos = app.cursor_pos.saturating_sub(1),
            KeyCode::Right => app.cursor_pos = (app.cursor_pos + 1).min(app.input.chars().count()),
            KeyCode::Home => app.cursor_pos = 0,
            KeyCode::End => app.cursor_pos = app.input.chars().count(),
            KeyCode::Char(c) => {
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
        if app.focus == Focus::Chat {
            match kind {
                MouseEventKind::ScrollUp => {
                    if !app.user_scrolled { app.scroll_offset = app.chat_max_scroll; }
                    app.scroll_offset = app.scroll_offset.saturating_sub(3);
                    app.user_scrolled = true;
                }
                MouseEventKind::ScrollDown => {
                    app.scroll_offset = app.scroll_offset.saturating_add(3);
                    if app.scroll_offset >= app.chat_max_scroll { app.user_scrolled = false; } else { app.user_scrolled = true; }
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
        Line::from(vec![Span::styled("  Navigation", Style::default().add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::styled("    Tab       ", Style::default().fg(Color::Yellow)), Span::raw(" Switch focus between Chat and Tasks")]),
        Line::from(vec![Span::styled("    Up/Down   ", Style::default().fg(Color::Yellow)), Span::raw(" Scroll chat or task list")]),
        Line::from(""),
        Line::from(vec![Span::styled("  Commands", Style::default().add_modifier(Modifier::BOLD))]),
        Line::from(vec![Span::styled("    Enter     ", Style::default().fg(Color::Yellow)), Span::raw(" Send message")]),
        Line::from(vec![Span::styled("    Ctrl+M    ", Style::default().fg(Color::Yellow)), Span::raw(" Open Unified Menu (Sessions, Models, Providers, Settings)")]),
        Line::from(vec![Span::styled("    Ctrl+R    ", Style::default().fg(Color::Yellow)), Span::raw(" Toggle Reasoning visibility")]),
        Line::from(vec![Span::styled("    F1        ", Style::default().fg(Color::Yellow)), Span::raw(" Show this help menu")]),
        Line::from(vec![Span::styled("    Esc/Ctrl+C", Style::default().fg(Color::Yellow)), Span::raw(" Quit Seekr")]),
        Line::from(""),
        Line::from(vec![Span::raw("  Press "), Span::styled("any key", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)), Span::raw(" to close")]),
    ];

    frame.render_widget(Paragraph::new(text).block(block), dialog_area);
} // render_help_dialog
