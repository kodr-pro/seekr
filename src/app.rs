// app.rs - Application state, event loop, and mode management
//
// This is the heart of the TUI application. It manages:
// - Application modes (Setup, Main, QuitConfirm)
// - The TUI event loop (terminal events + agent events)
// - Chat state and rendering coordination
// - Input handling and keybindings

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
use crate::api::client::DeepSeekClient;
use crate::config::AppConfig;
use crate::tools::task::Task;
use crate::tools::ActivityEntry;
use crate::ui;

/// Application mode
#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Setup,
    Main,
    QuitConfirm,
    /// Agent reached max iterations, waiting for user to Continue or Answer Now
    AwaitingContinue,
    /// Session list view for resuming or deleting sessions
    SessionList,
    /// Help menu showing keyboard shortcuts
    Help,
}

/// Focus area in the main view
#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Chat,
    Tasks,
}

/// Input mode — normal chat vs shell waiting for stdin
#[derive(Debug, Clone)]
pub enum InputMode {
    /// Normal chat with the agent
    Normal,
    /// A shell command is waiting for user input (e.g. sudo password, y/n)
    ShellStdin {
        /// Last few lines of output from the process, for context
        context: String,
        /// Channel to send the user's response to the process stdin
        input_tx: tokio::sync::mpsc::UnboundedSender<String>,
    },
}

impl PartialEq for InputMode {
    fn eq(&self, other: &Self) -> bool {
        matches!((self, other), (InputMode::Normal, InputMode::Normal) | (InputMode::ShellStdin { .. }, InputMode::ShellStdin { .. }))
    }
}

/// A chat entry displayed in the chat panel
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

/// State for the setup wizard
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

impl Default for SetupState {
    fn default() -> Self {
        Self {
            current_step: 0,
            api_key_input: String::new(),
            model_selection: 0,
            auto_approve_selection: 0,
            working_dir_input: String::new(),
            error_message: None,
            validating: false,
        }
    }
}

/// Main application state
pub struct App {
    pub mode: AppMode,
    pub focus: Focus,
    pub config: Option<AppConfig>,

    // Chat state
    pub chat_entries: Vec<ChatEntry>,
    pub input: String,
    pub cursor_pos: usize,
    pub scroll_offset: u16,
    pub chat_max_scroll: u16,
    pub user_scrolled: bool,
    pub show_reasoning: bool,

    // Agent communication channels
    pub agent_cmd_tx: Option<mpsc::UnboundedSender<AgentCommand>>,
    pub agent_event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,

    // Status
    pub total_tokens: u32,
    pub iteration: u32,
    pub connected: bool,
    pub awaiting_approval: bool,
    pub input_mode: InputMode,

    // Tasks and activity (mirrored from agent for UI rendering)
    pub tasks: Vec<Task>,
    pub activities: Vec<ActivityEntry>,

    // Streaming buffer
    pub streaming_content: String,
    pub streaming_reasoning: String,
    pub is_streaming: bool,

    // Manager
    pub manager: Option<std::sync::Arc<crate::manager::SeekrManager>>,

    // Setup wizard state
    pub setup_state: SetupState,

    // Resumption state
    pub session_id: Option<String>,

    // Session list state
    pub sessions: Vec<crate::session::SessionMetadata>,
    pub session_selection: usize,
    pub session_list_error: Option<String>,
}

impl App {
    /// Create a new app in setup mode
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
            streaming_content: String::new(),
            streaming_reasoning: String::new(),
            is_streaming: false,
            setup_state: SetupState::default(),
            session_id: None,
            sessions: Vec::new(),
            session_selection: 0,
            session_list_error: None,
        }
    }

    /// Create a new app in main mode with an existing config
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
    }

    /// Start the agent background task
    pub fn start_agent(&mut self) {
        if let Some(ref config) = self.config {
            let (evt_tx, evt_rx) = mpsc::unbounded_channel();
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

            let agent_res = if let Some(ref sid) = self.session_id {
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
        }
    }

    /// Set session ID for resumption
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
    }

    /// Send a user message to the agent
    pub fn send_message(&mut self) {
        if self.input.trim().is_empty() {
            return;
        }
        let msg = self.input.clone();
        self.input.clear();
        self.cursor_pos = 0;

        // If a shell process is waiting for input, forward directly to its stdin
        if let InputMode::ShellStdin { ref input_tx, .. } = self.input_mode.clone() {
            let _ = input_tx.send(msg);
            self.input_mode = InputMode::Normal;
            return;
        }

        self.chat_entries.push(ChatEntry::UserMessage(msg.clone()));
        self.is_streaming = true;
        self.streaming_content.clear();
        self.streaming_reasoning.clear();
        // New message — reset manual scroll so we auto-follow again
        self.user_scrolled = false;

        if let Some(ref tx) = self.agent_cmd_tx {
            tx.send(AgentCommand::UserMessage(msg)).ok();
        }

        // Auto-scroll to bottom
        self.scroll_offset = self.chat_max_scroll;
    }

    /// Process pending agent events (non-blocking)
    pub fn poll_agent_events(&mut self) {
        // Drain events into a temporary vec to avoid borrow conflicts
        let events: Vec<AgentEvent> = {
            let rx: &mut mpsc::UnboundedReceiver<AgentEvent> = match self.agent_event_rx.as_mut() {
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
                    self.chat_entries.push(ChatEntry::ToolCall {
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
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
                    self.activities.push(entry);
                }
                AgentEvent::TokenUsage {
                    total_tokens,
                    ..
                } => {
                    self.total_tokens = total_tokens;
                }
                AgentEvent::IterationUpdate(n) => {
                    self.iteration = n;
                }
                AgentEvent::TurnComplete => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.iteration = 0;
                    // Return to Main mode if we were in AwaitingContinue
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
                    name,
                    arguments,
                    ..
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
                AgentEvent::TaskCreated(task) => {
                    // Add or replace task
                    if let Some(pos) = self.tasks.iter().position(|t| t.id == task.id) {
                        self.tasks[pos] = task;
                    } else {
                        self.tasks.push(task);
                    }
                }
                AgentEvent::TaskUpdated(task) => {
                    // Update existing task
                    if let Some(pos) = self.tasks.iter().position(|t| t.id == task.id) {
                        self.tasks[pos] = task;
                    }
                }
            }
        }
    }

    /// Update the streaming content entry in chat
    fn update_streaming_entry(&mut self) {
        // Replace or add streaming entry
        if let Some(last) = self.chat_entries.last_mut() {
            if matches!(last, ChatEntry::AssistantStreaming(_)) {
                *last = ChatEntry::AssistantStreaming(
                    self.streaming_content.clone(),
                );
                return;
            }
        }
        self.chat_entries.push(ChatEntry::AssistantStreaming(
            self.streaming_content.clone(),
        ));
    }

    /// Update the reasoning entry in chat
    fn update_reasoning_entry(&mut self) {
        // Find and update existing reasoning entry, or add new one
        let len = self.chat_entries.len();
        for i in (0..len).rev() {
            if matches!(&self.chat_entries[i], ChatEntry::Reasoning(_)) {
                self.chat_entries[i] =
                    ChatEntry::Reasoning(self.streaming_reasoning.clone());
                return;
            }
            // Stop looking if we hit a non-reasoning/streaming entry
            if !matches!(
                &self.chat_entries[i],
                ChatEntry::AssistantStreaming(_) | ChatEntry::Reasoning(_)
            ) {
                break;
            }
        }
        // Insert reasoning before the streaming content
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
    }

    /// Finalize streaming content into a permanent chat entry
    fn finalize_streaming(&mut self) {
        if !self.streaming_content.is_empty() {
            // Replace the streaming entry with a finalized one, or append if not found
            if let Some(pos) = self
                .chat_entries
                .iter()
                .rposition(|e| matches!(e, ChatEntry::AssistantStreaming(_)))
            {
                self.chat_entries[pos] =
                    ChatEntry::AssistantContent(self.streaming_content.clone());
            } else {
                self.chat_entries.push(ChatEntry::AssistantContent(
                    self.streaming_content.clone(),
                ));
            }
            self.streaming_content.clear();
        }
        
        // Finalize reasoning as well if it exists
        if !self.streaming_reasoning.is_empty() {
            if let Some(_pos) = self
                .chat_entries
                .iter()
                .rposition(|e| matches!(e, ChatEntry::Reasoning(_)))
            {
                // We keep it as Reasoning entry but it's now "stable" 
                // since we cleared the reasoning buffer.
            }
            self.streaming_reasoning.clear();
        }
    }

    /// Handle tool approval input
    pub fn handle_approval(&mut self, approved: bool, always: bool) {
        self.awaiting_approval = false;
        // Remove the approval prompt from chat
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
    }

    /// Clear chat history
    pub fn clear_chat(&mut self) {
        self.chat_entries.clear();
        self.chat_entries.push(ChatEntry::SystemInfo(
            "Chat cleared. Type a message to continue.".to_string(),
        ));
        self.scroll_offset = 0;
    }

    /// Load session list from disk
    pub async fn load_sessions(&mut self) {
        if let Some(ref mgr) = self.manager {
            if let Err(e) = mgr.load_sessions().await {
                self.session_list_error = Some(format!("Failed to load sessions: {}", e));
                self.sessions.clear();
            } else {
                self.sessions = mgr.list_sessions().await;
                self.session_list_error = None;
            }
        } else {
            // Fallback for when manager is not yet initialized (e.g. during setup)
            match crate::session::Session::list_all() {
                Ok(sessions) => {
                    self.sessions = sessions;
                    self.session_list_error = None;
                }
                Err(e) => {
                    self.session_list_error = Some(format!("Failed to load sessions: {}", e));
                    self.sessions.clear();
                }
            }
        }
    }

    /// Switch to session list mode
    pub async fn show_session_list(&mut self) {
        self.load_sessions().await;
        self.mode = AppMode::SessionList;
        self.session_selection = 0;
    }

    /// Resume a selected session
    pub fn resume_selected_session(&mut self) {
        if let Some(session) = self.sessions.get(self.session_selection) {
            let id = session.id.clone();
            self.session_id = Some(id.clone());
            self.mode = AppMode::Main;
            self.resume_session(id);
            self.start_agent();
        }
    }

    /// Delete the selected session
    pub async fn delete_selected_session(&mut self) {
        if let Some(session) = self.sessions.get(self.session_selection) {
            let id = session.id.clone();
            if let Some(ref mgr) = self.manager {
                if let Err(e) = mgr.delete_session(&id).await {
                    self.session_list_error = Some(format!("Failed to delete session: {}", e));
                    return;
                }
                // Refresh list
                self.sessions = mgr.list_sessions().await;
            } else {
                // Fallback
                let dir = match crate::session::Session::sessions_dir() {
                    Ok(dir) => dir,
                    Err(e) => {
                        self.session_list_error = Some(format!("Failed to get sessions directory: {}", e));
                        return;
                    }
                };
                let path = dir.join(format!("{}.json", id));
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                self.sessions.retain(|s| s.id != id);
            }

            if self.session_selection >= self.sessions.len() && !self.sessions.is_empty() {
                self.session_selection = self.sessions.len() - 1;
            }
        }
    }
}

/// Run the full TUI event loop
pub async fn run_app(mut app: App) -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);

    // If starting in main mode, start the agent
    if app.mode == AppMode::Main {
        app.start_agent();
    }

    let result = event_loop(&mut terminal, &mut app).await;

    let _ = ratatui::crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();

    // Shutdown agent if running
    if let Some(ref tx) = app.agent_cmd_tx {
        tx.send(AgentCommand::Shutdown).ok();
    }

    result
}

/// Main event loop: polls terminal events and agent events
async fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
) -> Result<()> {
    loop {
        // Render
        terminal.draw(|frame| render(frame, app))?;

        // Poll agent events (non-blocking) in both Main and AwaitingContinue
        if app.mode == AppMode::Main || app.mode == AppMode::AwaitingContinue {
            app.poll_agent_events();
        }

        // Poll terminal events with a short timeout for responsiveness
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
                AppMode::SessionList | AppMode::Help => {
                    // Session list and Help keybindings handled here
                    if let crossterm::event::Event::Key(key) = &ev {
                        match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => {
                                app.mode = AppMode::Main;
                            }
                            _ => {
                                if app.mode == AppMode::Help {
                                    app.mode = AppMode::Main;
                                } else if app.mode == AppMode::SessionList {
                                     match key.code {
                                        KeyCode::Up => {
                                            app.session_selection = app.session_selection.saturating_sub(1);
                                        }
                                        KeyCode::Down => {
                                            app.session_selection = (app.session_selection + 1)
                                                .min(app.sessions.len().saturating_sub(1));
                                        }
                                        KeyCode::Enter => {
                                            app.resume_selected_session();
                                        }
                                        KeyCode::Char('d') | KeyCode::Delete => {
                                            app.delete_selected_session().await;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Render the current application state
fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    match app.mode {
        AppMode::Setup => {
            ui::setup::render_setup(frame, area, &app.setup_state);
        }
        AppMode::Main | AppMode::QuitConfirm | AppMode::AwaitingContinue => {
            render_main(frame, area, app);
            if app.mode == AppMode::QuitConfirm {
                render_quit_dialog(frame, area);
            }
            if app.mode == AppMode::AwaitingContinue {
                render_continue_dialog(frame, area);
            }
        }
        AppMode::SessionList => {
            // Placeholder: render main layout behind a session list overlay
            render_main(frame, area, app);
        }
        AppMode::Help => {
            render_main(frame, area, app);
            render_help_dialog(frame, area);
        }
    }
}

/// Render the main application layout
fn render_main(frame: &mut Frame, area: Rect, app: &mut App) {
    let layout = ui::layout::AppLayout::new(area);

    // Title bar
    render_title_bar(frame, layout.title_bar, app);

    // Chat panel
    app.chat_max_scroll = ui::chat::render_chat(
        frame,
        layout.chat_panel,
        &app.chat_entries,
        app.scroll_offset,
        app.focus == Focus::Chat,
    );

    // Task panel
    ui::tasks::render_tasks(
        frame,
        layout.task_panel,
        &app.tasks,
        &app.activities,
        app.focus == Focus::Tasks,
    );

    // Input bar
    let (shell_context, input_prompt) = match &app.input_mode {
        InputMode::ShellStdin { context, .. } => {
            let ctx = if context.is_empty() { None } else { Some(context.as_str()) };
            (ctx, Some("Shell input required"))
        }
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

    // Status bar
    let model = app
        .config
        .as_ref()
        .map(|c| c.api.model.as_str())
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
            model,
            total_tokens: app.total_tokens,
            iteration: app.iteration,
            max_iterations: max_iter,
            is_thinking: app.is_streaming,
        },
    );
}

fn render_title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let model = app
        .config
        .as_ref()
        .map(|c| c.api.model.as_str())
        .unwrap_or("unknown");

    let status = if app.is_streaming {
        "Working"
    } else {
        "Ready"
    };

    let title_info = ui::title::TitleInfo {
        version: "0.1.0",
        session_id: app.session_id.as_deref(),
        connected: app.connected,
        model,
        status,
    };

    ui::title::render_title(frame, area, &title_info);
}

fn render_quit_dialog(frame: &mut Frame, area: Rect) {
    let dialog_width = 44u16;
    let dialog_height = 7u16;
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(
        x,
        y,
        dialog_width.min(area.width),
        dialog_height.min(area.height),
    );

    // Clear the dialog area and draw a block with a solid background
    frame.render_widget(ratatui::widgets::Clear, dialog_area);
    
    let block = ratatui::widgets::Block::default()
        .title(" Confirmation ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .style(Style::default().bg(Color::Reset)); // Use Reset or specific background if needed

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "    Are you sure you want to quit?",
            Style::default()
                .fg(Color::White)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("    Press "),
            Span::styled("[Y]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to Yes, "),
            Span::styled("[N]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to No"),
        ]),
    ];

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, dialog_area);
}

fn render_continue_dialog(frame: &mut Frame, area: Rect) {
    let dialog_width = 58u16;
    let dialog_height = 8u16;
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(
        x,
        y,
        dialog_width.min(area.width),
        dialog_height.min(area.height),
    );

    frame.render_widget(ratatui::widgets::Clear, dialog_area);

    let block = ratatui::widgets::Block::default()
        .title(" Max Iterations Reached ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Reset));

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  The agent has used all available iterations.",
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled("[C]", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::raw(" to Continue   "),
            Span::styled("[A]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw(" to Answer Now"),
        ]),
    ];

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, dialog_area);
}

/// Handle events in setup mode. Returns true if the app should exit.
async fn handle_setup_event(app: &mut App, ev: &Event) -> Result<bool> {
    if let Event::Key(key) = ev {
        match key.code {
            KeyCode::Char('c')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                return Ok(true);
            }
            _ => {}
        }

        match app.setup_state.current_step {
            // Welcome screen
            0 => {
                if key.code == KeyCode::Enter {
                    app.setup_state.current_step = 1;
                }
            }
            // API key input
            1 => match key.code {
                KeyCode::Enter => {
                    if !app.setup_state.api_key_input.is_empty() {
                        app.setup_state.error_message = None;
                        app.setup_state.current_step = 2;
                    } else {
                        app.setup_state.error_message =
                            Some("API key cannot be empty".to_string());
                    }
                }
                KeyCode::Esc => {
                    app.setup_state.current_step = 0;
                }
                KeyCode::Backspace => {
                    app.setup_state.api_key_input.pop();
                }
                KeyCode::Char(c) => {
                    app.setup_state.api_key_input.push(c);
                    app.setup_state.error_message = None;
                }
                _ => {}
            },
            // Model selection
            2 => match key.code {
                KeyCode::Up => {
                    app.setup_state.model_selection =
                        app.setup_state.model_selection.saturating_sub(1);
                }
                KeyCode::Down => {
                    app.setup_state.model_selection =
                        (app.setup_state.model_selection + 1).min(1);
                }
                KeyCode::Enter => {
                    app.setup_state.current_step = 3;
                }
                KeyCode::Esc => {
                    app.setup_state.current_step = 1;
                }
                _ => {}
            },
            // Auto-approve selection
            3 => match key.code {
                KeyCode::Up => {
                    app.setup_state.auto_approve_selection = app
                        .setup_state
                        .auto_approve_selection
                        .saturating_sub(1);
                }
                KeyCode::Down => {
                    app.setup_state.auto_approve_selection =
                        (app.setup_state.auto_approve_selection + 1).min(1);
                }
                KeyCode::Enter => {
                    app.setup_state.current_step = 4;
                }
                KeyCode::Esc => {
                    app.setup_state.current_step = 2;
                }
                _ => {}
            },
            // Working directory
            4 => match key.code {
                KeyCode::Enter => {
                    // Move to validation step
                    app.setup_state.current_step = 5;
                    app.setup_state.error_message = None;
                    app.setup_state.validating = true;

                    // Validate the API key
                    let key = app.setup_state.api_key_input.clone();
                    let valid = DeepSeekClient::validate_key(
                        &key,
                        "https://api.deepseek.com",
                    )
                    .await;
                    app.setup_state.validating = false;

                    match valid {
                        Ok(true) => {
                            // Build and save config
                            let model = if app.setup_state.model_selection == 0
                            {
                                "deepseek-chat"
                            } else {
                                "deepseek-reasoner"
                            };
                            let auto_approve =
                                app.setup_state.auto_approve_selection == 1;
                            let working_dir =
                                if app.setup_state.working_dir_input.is_empty()
                                {
                                    ".".to_string()
                                } else {
                                    app.setup_state.working_dir_input.clone()
                                };

                            let config = AppConfig {
                                api: crate::config::ApiConfig {
                                    key: app.setup_state.api_key_input.clone(),
                                    model: model.to_string(),
                                    base_url: "https://api.deepseek.com"
                                        .to_string(),
                                },
                                agent: crate::config::AgentConfig {
                                    max_iterations: 15,
                                    auto_approve_tools: auto_approve,
                                    working_directory: working_dir,
                                },
                                ui: crate::config::UiConfig {
                                    theme: "dark".to_string(),
                                    show_reasoning: true,
                                },
                            };

                            if let Err(e) = config.save() {
                                app.setup_state.error_message = Some(format!(
                                    "Failed to save config: {}",
                                    e
                                ));
                            } else {
                                app.config = Some(config);
                                app.setup_state.current_step = 6; // success
                            }
                        }
                        Ok(false) => {
                            app.setup_state.error_message = Some(
                                "Invalid API key. Please check and try again."
                                    .to_string(),
                            );
                        }
                        Err(e) => {
                            app.setup_state.error_message =
                                Some(format!("Connection error: {}", e));
                        }
                    }
                }
                KeyCode::Esc => {
                    app.setup_state.current_step = 3;
                }
                KeyCode::Backspace => {
                    app.setup_state.working_dir_input.pop();
                }
                KeyCode::Char(c) => {
                    app.setup_state.working_dir_input.push(c);
                }
                _ => {}
            },
            // Validation step (error state)
            5 => {
                if key.code == KeyCode::Enter {
                    // Go back to API key step
                    app.setup_state.current_step = 1;
                    app.setup_state.error_message = None;
                }
            }
            // Complete - transition to main
            6 => {
                if key.code == KeyCode::Enter {
                    app.mode = AppMode::Main;
                    app.show_reasoning = true;
                    app.chat_entries.push(ChatEntry::SystemInfo(
                        "Welcome to Seekr! Type a message to start."
                            .to_string(),
                    ));
                    app.start_agent();
                }
            }
            _ => {}
        }
    }
    Ok(false)
}

/// Handle events in main mode. Returns true if the app should exit.
pub async fn handle_main_event(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(KeyEvent {
        code, modifiers, ..
    }) = ev
    {
        // Ctrl+C: force quit
        if *code == KeyCode::Char('c')
            && modifiers.contains(KeyModifiers::CONTROL)
        {
            return true;
        }

        // Ctrl+S: Session List
        if *code == KeyCode::Char('s')
            && modifiers.contains(KeyModifiers::CONTROL)
        {
            app.show_session_list().await;
            return false;
        }

        if app.mode == AppMode::Help {
            app.mode = AppMode::Main;
            return false;
        }

        // AwaitingContinue popup mode
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

        // Tool approval mode
        if app.awaiting_approval {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    app.handle_approval(true, false);
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    app.handle_approval(false, false);
                }
                KeyCode::Char('a') | KeyCode::Char('A') => {
                    app.handle_approval(true, true);
                }
                _ => {}
            }
            return false;
        }

        match code {
            KeyCode::F(1) => {
                app.mode = AppMode::Help;
            }
            KeyCode::Enter => {
                app.send_message();
                app.user_scrolled = false;
                app.scroll_offset = app.chat_max_scroll;
            }
            KeyCode::Esc => {
                app.mode = AppMode::QuitConfirm;
            }
            KeyCode::Tab => {
                app.focus = match app.focus {
                    Focus::Chat => Focus::Tasks,
                    Focus::Tasks => Focus::Chat,
                };
            }
            KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.clear_chat();
            }
            KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
                app.show_reasoning = !app.show_reasoning;
            }
            KeyCode::PageUp if app.focus == Focus::Chat => {
                if !app.user_scrolled {
                    app.scroll_offset = app.chat_max_scroll;
                }
                app.scroll_offset = app.scroll_offset.saturating_sub(10);
                app.user_scrolled = true;
            }
            KeyCode::PageDown if app.focus == Focus::Chat => {
                app.scroll_offset = app.scroll_offset.saturating_add(10);
                if app.scroll_offset >= app.chat_max_scroll {
                    app.user_scrolled = false;
                }
            }
            KeyCode::Up if app.focus == Focus::Chat => {
                if !app.user_scrolled {
                    app.scroll_offset = app.chat_max_scroll;
                }
                app.scroll_offset = app.scroll_offset.saturating_sub(1);
                app.user_scrolled = true;
            }
            KeyCode::Down if app.focus == Focus::Chat => {
                app.scroll_offset = app.scroll_offset.saturating_add(1);
                if app.scroll_offset >= app.chat_max_scroll {
                    app.user_scrolled = false;
                }
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
            KeyCode::Left => {
                app.cursor_pos = app.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                let char_count = app.input.chars().count();
                app.cursor_pos = (app.cursor_pos + 1).min(char_count);
            }
            KeyCode::Home => {
                app.cursor_pos = 0;
            }
            KeyCode::End => {
                app.cursor_pos = app.input.chars().count();
            }
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
}

/// Handle quit confirmation dialog
fn handle_quit_confirm(app: &mut App, ev: &Event) -> bool {
    if let Event::Key(KeyEvent { code, .. }) = ev {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                return true;
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.mode = AppMode::Main;
            }
            _ => {}
        }
    }
    false
}

fn render_help_dialog(frame: &mut Frame, area: Rect) {
    let dialog_width = 60u16;
    let dialog_height = 14u16;
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(
        x,
        y,
        dialog_width.min(area.width),
        dialog_height.min(area.height),
    );

    frame.render_widget(ratatui::widgets::Clear, dialog_area);
    
    let block = ratatui::widgets::Block::default()
        .title(" Seekr Help & Shortcuts ")
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Navigation", Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    Tab       ", Style::default().fg(Color::Yellow)),
            Span::raw(" Switch focus between Chat and Tasks"),
        ]),
        Line::from(vec![
            Span::styled("    Up/Down   ", Style::default().fg(Color::Yellow)),
            Span::raw(" Scroll chat or task list"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Commands", Style::default().add_modifier(Modifier::BOLD)),
        ]),
        Line::from(vec![
            Span::styled("    Enter     ", Style::default().fg(Color::Yellow)),
            Span::raw(" Send message"),
        ]),
        Line::from(vec![
            Span::styled("    Ctrl+S    ", Style::default().fg(Color::Yellow)),
            Span::raw(" Open Session List"),
        ]),
        Line::from(vec![
            Span::styled("    Ctrl+R    ", Style::default().fg(Color::Yellow)),
            Span::raw(" Toggle Reasoning visibility"),
        ]),
        Line::from(vec![
            Span::styled("    F1        ", Style::default().fg(Color::Yellow)),
            Span::raw(" Show this help menu"),
        ]),
        Line::from(vec![
            Span::styled("    Esc/Ctrl+C", Style::default().fg(Color::Yellow)),
            Span::raw(" Quit Seekr"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled("any key", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to close"),
        ]),
    ];

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, dialog_area);
}
