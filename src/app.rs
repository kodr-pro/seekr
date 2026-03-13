// app.rs - Application state, event loop, and mode management
//
// This is the heart of the TUI application. It manages:
// - Application modes (Setup, Main, QuitConfirm)
// - The TUI event loop (terminal events + agent events)
// - Chat state and rendering coordination
// - Input handling and keybindings

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use std::time::Duration;
use tokio::sync::mpsc;

use crate::agent::{AgentCommand, AgentEvent, AgentLoop};
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
}

/// Focus area in the main view
#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Chat,
    Tasks,
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
            working_dir_input: ".".to_string(),
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
    pub show_reasoning: bool,

    // Agent communication channels
    pub agent_cmd_tx: Option<mpsc::UnboundedSender<AgentCommand>>,
    pub agent_event_rx: Option<mpsc::UnboundedReceiver<AgentEvent>>,

    // Status
    pub total_tokens: u32,
    pub iteration: u32,
    pub connected: bool,
    pub awaiting_approval: bool,

    // Tasks and activity (mirrored from agent for UI rendering)
    pub tasks: Vec<Task>,
    pub activities: Vec<ActivityEntry>,

    // Streaming buffer
    pub streaming_content: String,
    pub streaming_reasoning: String,
    pub is_streaming: bool,

    // Setup wizard state
    pub setup_state: SetupState,
}

impl App {
    /// Create a new app in setup mode
    pub fn new_setup() -> Self {
        Self {
            mode: AppMode::Setup,
            focus: Focus::Chat,
            config: None,
            chat_entries: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            show_reasoning: true,
            agent_cmd_tx: None,
            agent_event_rx: None,
            total_tokens: 0,
            iteration: 0,
            connected: false,
            awaiting_approval: false,
            tasks: Vec::new(),
            activities: Vec::new(),
            streaming_content: String::new(),
            streaming_reasoning: String::new(),
            is_streaming: false,
            setup_state: SetupState::default(),
        }
    }

    /// Create a new app in main mode with an existing config
    pub fn new_main(config: AppConfig) -> Self {
        let show_reasoning = config.ui.show_reasoning;
        let mut app = Self {
            mode: AppMode::Main,
            config: Some(config),
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

            let agent = AgentLoop::new(config.clone(), evt_tx, cmd_rx);
            tokio::spawn(agent.run());

            self.agent_cmd_tx = Some(cmd_tx);
            self.agent_event_rx = Some(evt_rx);
            self.connected = true;
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

        self.chat_entries.push(ChatEntry::UserMessage(msg.clone()));
        self.is_streaming = true;
        self.streaming_content.clear();
        self.streaming_reasoning.clear();

        if let Some(ref tx) = self.agent_cmd_tx {
            let _ = tx.send(AgentCommand::UserMessage(msg));
        }

        // Auto-scroll to bottom
        self.scroll_offset = u16::MAX;
    }

    /// Process pending agent events (non-blocking)
    pub fn poll_agent_events(&mut self) {
        // Drain events into a temporary vec to avoid borrow conflicts
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
                    self.scroll_offset = u16::MAX;
                }
                AgentEvent::ReasoningDelta(text) => {
                    self.streaming_reasoning.push_str(&text);
                    if self.show_reasoning {
                        self.update_reasoning_entry();
                    }
                    self.scroll_offset = u16::MAX;
                }
                AgentEvent::ToolCallStart { name, arguments } => {
                    self.finalize_streaming();
                    self.chat_entries.push(ChatEntry::ToolCall {
                        name: name.clone(),
                        arguments: arguments.clone(),
                    });
                    self.scroll_offset = u16::MAX;
                }
                AgentEvent::ToolCallResult { name, result } => {
                    self.chat_entries
                        .push(ChatEntry::ToolResult { name, result });
                    self.scroll_offset = u16::MAX;
                    self.streaming_content.clear();
                    self.streaming_reasoning.clear();
                }
                AgentEvent::Activity(entry) => {
                    self.activities.push(entry);
                }
                AgentEvent::TokenUsage {
                    total_tokens,
                    prompt_tokens: _,
                    completion_tokens: _,
                } => {
                    self.total_tokens = total_tokens;
                }
                AgentEvent::TurnComplete => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.iteration = 0;
                }
                AgentEvent::MaxIterationsReached => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.chat_entries.push(ChatEntry::SystemInfo(
                        "Agent reached maximum iterations. Send another message to continue."
                            .to_string(),
                    ));
                }
                AgentEvent::Error(msg) => {
                    self.finalize_streaming();
                    self.is_streaming = false;
                    self.chat_entries.push(ChatEntry::Error(msg));
                }
                AgentEvent::ToolApprovalRequest {
                    name,
                    arguments,
                    call_index: _,
                } => {
                    self.awaiting_approval = true;
                    self.chat_entries
                        .push(ChatEntry::ToolApproval { name, arguments });
                    self.scroll_offset = u16::MAX;
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
            // Replace the streaming entry with a finalized one
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
        }
        self.streaming_content.clear();
        self.streaming_reasoning.clear();
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
                let _ = tx.send(AgentCommand::ToolAlwaysApprove);
            } else if approved {
                let _ = tx.send(AgentCommand::ToolApproved { call_index: 0 });
            } else {
                let _ = tx.send(AgentCommand::ToolDenied { call_index: 0 });
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
}

/// Run the full TUI event loop
pub async fn run_app(mut app: App) -> Result<()> {
    let mut terminal = ratatui::init();

    // If starting in main mode, start the agent
    if app.mode == AppMode::Main {
        app.start_agent();
    }

    let result = event_loop(&mut terminal, &mut app).await;

    ratatui::restore();

    // Shutdown agent if running
    if let Some(ref tx) = app.agent_cmd_tx {
        let _ = tx.send(AgentCommand::Shutdown);
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

        // Poll agent events (non-blocking)
        if app.mode == AppMode::Main {
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
                AppMode::Main => {
                    if handle_main_event(app, &ev) {
                        return Ok(());
                    }
                }
                AppMode::QuitConfirm => {
                    if handle_quit_confirm(app, &ev) {
                        return Ok(());
                    }
                }
            }
        }
    }
}

/// Render the current application state
fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    match app.mode {
        AppMode::Setup => {
            ui::setup::render_setup(frame, area, &app.setup_state);
        }
        AppMode::Main | AppMode::QuitConfirm => {
            render_main(frame, area, app);
            if app.mode == AppMode::QuitConfirm {
                render_quit_dialog(frame, area);
            }
        }
    }
}

/// Render the main application layout
fn render_main(frame: &mut Frame, area: Rect, app: &App) {
    let layout = ui::layout::AppLayout::new(area);

    // Title bar
    render_title_bar(frame, layout.title_bar, app);

    // Chat panel
    ui::chat::render_chat(
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
    ui::input::render_input(
        frame,
        layout.input_bar,
        &app.input,
        app.cursor_pos,
        !app.awaiting_approval,
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
        .unwrap_or(25);
    ui::status::render_status(
        frame,
        layout.status_bar,
        &ui::status::StatusInfo {
            connected: app.connected,
            model,
            total_tokens: app.total_tokens,
            iteration: app.iteration,
            max_iterations: max_iter,
        },
    );
}

fn render_title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let model = app
        .config
        .as_ref()
        .map(|c| c.api.model.as_str())
        .unwrap_or("unknown");

    let line = Line::from(vec![
        Span::styled(
            " Seekr Agent ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("[{}]", model),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        if app.is_streaming {
            Span::styled("working...", Style::default().fg(Color::Yellow))
        } else {
            Span::styled("ready", Style::default().fg(Color::Green))
        },
    ]);

    let paragraph =
        Paragraph::new(line).style(Style::default().bg(Color::DarkGray));
    frame.render_widget(paragraph, area);
}

fn render_quit_dialog(frame: &mut Frame, area: Rect) {
    let dialog_width = 40u16;
    let dialog_height = 5u16;
    let x = area.width.saturating_sub(dialog_width) / 2;
    let y = area.height.saturating_sub(dialog_height) / 2;
    let dialog_area = Rect::new(
        x,
        y,
        dialog_width.min(area.width),
        dialog_height.min(area.height),
    );

    frame.render_widget(ratatui::widgets::Clear, dialog_area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Quit Seekr?",
            Style::default()
                .fg(Color::White)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [Y]es  /  [N]o",
            Style::default().fg(Color::Yellow),
        )),
    ];

    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(Color::Red));

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
                                    max_iterations: 25,
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
fn handle_main_event(app: &mut App, ev: &Event) -> bool {
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
            KeyCode::Enter => {
                app.send_message();
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
            KeyCode::PageUp => {
                app.scroll_offset = app.scroll_offset.saturating_sub(10);
            }
            KeyCode::PageDown => {
                app.scroll_offset = app.scroll_offset.saturating_add(10);
            }
            KeyCode::Backspace => {
                if app.cursor_pos > 0 {
                    app.input.remove(app.cursor_pos - 1);
                    app.cursor_pos -= 1;
                }
            }
            KeyCode::Delete => {
                if app.cursor_pos < app.input.len() {
                    app.input.remove(app.cursor_pos);
                }
            }
            KeyCode::Left => {
                app.cursor_pos = app.cursor_pos.saturating_sub(1);
            }
            KeyCode::Right => {
                app.cursor_pos = (app.cursor_pos + 1).min(app.input.len());
            }
            KeyCode::Home => {
                app.cursor_pos = 0;
            }
            KeyCode::End => {
                app.cursor_pos = app.input.len();
            }
            KeyCode::Char(c) => {
                app.input.insert(app.cursor_pos, *c);
                app.cursor_pos += 1;
            }
            _ => {}
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
