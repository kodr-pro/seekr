use crate::app::{App, AppMode, Focus, InputMode};
use crate::ui;
use ratatui::{
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, app: &mut App) {
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
}

fn render_main(frame: &mut Frame, area: Rect, app: &mut App) {
    let layout = ui::layout::AppLayout::new(area);
    app.ui.layout = Some(layout.clone());

    render_title_bar(frame, layout.title_bar, app);

    let inner_chat = layout.chat_panel.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    // Recompute visual lines if width changed or explicitly requested
    if app.ui.needs_recompute_vlines || app.ui.last_chat_width != inner_chat.width {
        app.rebuild_vlines_cache(inner_chat.width);
    }

    app.ui.chat_max_scroll = ui::chat::render_chat(
        frame,
        layout.chat_panel,
        &app.visual_lines,
        app.ui.scroll_offset,
        app.focus == Focus::Chat,
    );

    ui::tasks::render_tasks(
        frame,
        layout.task_panel,
        &app.tasks,
        &app.agent.activities,
        &app.agent.live_activities,
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
        _ => (None, None),
    };

    ui::input::render_input(
        frame,
        layout.input_bar,
        &app.input,
        app.cursor_pos,
        app.focus == Focus::Input && !app.agent.awaiting_approval,
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
            session_id: app.session.session_id.as_deref().unwrap_or("none"),
            connected: app.agent.connected,
            provider,
            model,
            total_tokens: app.agent.total_tokens,
            iteration: app.agent.iteration,
            max_iterations: max_iter,
            is_thinking: app.agent.is_streaming,
        },
    );
}

fn render_title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let model = app
        .config
        .as_ref()
        .map(|c| c.current_provider().model.as_str())
        .unwrap_or("unknown");
    let status = if app.agent.is_streaming {
        "Working"
    } else {
        "Ready"
    };

    ui::title::render_title(
        frame,
        area,
        &ui::title::TitleInfo {
            version: env!("CARGO_PKG_VERSION"),
            new_version: app.new_version_available.as_deref(),
            session_id: app.session.session_id.as_deref(),
            connected: app.agent.connected,
            model,
            status,
        },
    );
}

fn render_quit_dialog(frame: &mut Frame, area: Rect) {
    let dialog_area = centered_rect(44, 7, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(" Confirmation ")
        .borders(Borders::ALL)
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
}

fn render_continue_dialog(frame: &mut Frame, area: Rect) {
    let dialog_area = centered_rect(58, 8, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(" Max Iterations Reached ")
        .borders(Borders::ALL)
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
}

fn render_help_dialog(frame: &mut Frame, area: Rect) {
    let dialog_area = centered_rect(60, 15, area);
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(" Help / Shortcuts ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Ctrl-G", Style::default().fg(Color::Yellow)),
            Span::raw("  Open Unified Menu (Sessions, Models, Providers)"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl-R", Style::default().fg(Color::Yellow)),
            Span::raw("  Clear Chat History"),
        ]),
        Line::from(vec![
            Span::styled("  Ctrl-C", Style::default().fg(Color::Yellow)),
            Span::raw("  Quit Application"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Tab   ", Style::default().fg(Color::Yellow)),
            Span::raw("  Switch focus between panels"),
        ]),
        Line::from(vec![
            Span::styled("  Enter ", Style::default().fg(Color::Yellow)),
            Span::raw("  Send message"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Chat Panel Navigation (when focused):",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("    j / k   ", Style::default().fg(Color::Yellow)),
            Span::raw("Scroll down / up"),
        ]),
        Line::from(vec![
            Span::styled("    v / V   ", Style::default().fg(Color::Yellow)),
            Span::raw("Visual / Visual-Line mode"),
        ]),
        Line::from(vec![
            Span::styled("    y       ", Style::default().fg(Color::Yellow)),
            Span::raw("Copy selection to clipboard"),
        ]),
        Line::from(vec![
            Span::styled("    c       ", Style::default().fg(Color::Yellow)),
            Span::raw("Copy code block under cursor"),
        ]),
        Line::from(""),
        Line::from(vec![Span::raw("  Press any key to close this dialog")]),
    ];

    frame.render_widget(Paragraph::new(text).block(block), dialog_area);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}
