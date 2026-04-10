use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap},
};

use crate::app::{App, MenuTab};

pub fn render_help_tab(frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Menu Shortcuts ");

    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("    Tab / l / →  ", Style::default().fg(Color::Yellow)),
            Span::raw(" Next Tab"),
        ]),
        Line::from(vec![
            Span::styled("    h / ←        ", Style::default().fg(Color::Yellow)),
            Span::raw(" Previous Tab"),
        ]),
        Line::from(vec![
            Span::styled("    j / k / ↑/↓  ", Style::default().fg(Color::Yellow)),
            Span::raw(" Select Item"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Actions",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("    Enter        ", Style::default().fg(Color::Yellow)),
            Span::raw(" Activate / Switch / Toggle"),
        ]),
        Line::from(vec![
            Span::styled("    d / Delete   ", Style::default().fg(Color::Yellow)),
            Span::raw(" Delete Session (in Sessions tab)"),
        ]),
        Line::from(vec![
            Span::styled("    Esc / q      ", Style::default().fg(Color::Yellow)),
            Span::raw(" Close Menu"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  Tabs Overview",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from("    • Sessions  : Browse and resume previous chats"),
        Line::from("    • Models    : Select AI model for the active provider"),
        Line::from("    • Providers : Switch between configured API providers"),
        Line::from("    • Settings  : Toggle tools, iterations, and UI options"),
    ];

    frame.render_widget(Paragraph::new(text).block(block), area);
}

pub fn render_menu(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Seekr - Control Center ")
        .borders(Borders::ALL)
        .border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );

    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Tabs
            Constraint::Min(0),    // Content
            Constraint::Length(1), // Help Footer
        ])
        .split(inner_area);

    render_tabs(frame, chunks[0], app);
    render_content(frame, chunks[1], app);
    render_footer(frame, chunks[2], app);
} // render_menu

fn render_tabs(frame: &mut Frame, area: Rect, app: &App) {
    let titles = vec![
        " Sessions ",
        " Models ",
        " Providers ",
        " Skills ",
        " Settings ",
        " Help ",
    ];

    let selected_tab = match app.menu_state.active_tab {
        MenuTab::Sessions => 0,
        MenuTab::Models => 1,
        MenuTab::Providers => 2,
        MenuTab::Skills => 3,
        MenuTab::Settings => 4,
        MenuTab::Help => 5,
    };

    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .select(selected_tab)
        .style(Style::default().fg(Color::Gray))
        .highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .divider("|");

    frame.render_widget(tabs, area);
} // render_tabs

fn render_content(frame: &mut Frame, area: Rect, app: &App) {
    if !matches!(app.input_mode, crate::app::InputMode::Normal) {
        render_input_overlay(frame, area, app);
        return;
    }

    match app.menu_state.active_tab {
        MenuTab::Sessions => render_sessions(frame, area, app),
        MenuTab::Models => render_models(frame, area, app),
        MenuTab::Providers => render_providers(frame, area, app),
        MenuTab::Skills => render_skills(frame, area, app),
        MenuTab::Settings => render_settings(frame, area, app),
        MenuTab::Help => render_help(frame, area, app),
    }
} // render_content

fn render_input_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let (title, prompt) = match app.input_mode {
        crate::app::InputMode::EditingProviderKey { .. } => (" Edit API Key ", "Key: "),
        crate::app::InputMode::EditingProviderName { .. } => (" Edit Provider Name ", "Name: "),
        crate::app::InputMode::EditingProviderUrl { .. } => (" Edit Base URL ", "URL: "),
        crate::app::InputMode::EditingProviderModel { .. } => (" Edit Default Model ", "Model: "),
        _ => (" Edit Field ", "Value: "),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let overlay_area = centered_rect(60, 20, area);
    frame.render_widget(Clear, overlay_area);

    let inner = block.inner(overlay_area);
    frame.render_widget(block, overlay_area);

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(prompt, Style::default().fg(Color::Gray)),
            Span::styled(&app.input, Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            " [Enter] Save  [Esc] Cancel",
            Style::default().fg(Color::DarkGray),
        )]),
    ];

    frame.render_widget(Paragraph::new(text), inner);

    // Set cursor
    frame.set_cursor_position((
        inner.x + prompt.len() as u16 + app.cursor_pos as u16,
        inner.y + 1,
    ));
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_sessions(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = if app.session.sessions.is_empty() {
        vec![ListItem::new(" No sessions found.")]
    } else {
        app.session
            .sessions
            .iter()
            .map(|s| {
                let time_str = s.updated_at.format("%Y-%m-%d %H:%M").to_string();
                let content = format!(" {} ({})", s.title, time_str);
                ListItem::new(content)
            })
            .collect()
    };

    let list = List::new(items)
        .block(Block::default().title(" Sessions ").borders(Borders::NONE))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol(">> ");

    let mut state = ListState::default().with_selected(Some(app.menu_state.selection_idx));
    frame.render_stateful_widget(list, area, &mut state);
} // render_sessions

fn render_models(frame: &mut Frame, area: Rect, app: &App) {
    let items: Vec<ListItem> = if app.session.available_models.is_empty() {
        vec![ListItem::new(
            " Fetching models... (Press Ctrl+g again to refresh)",
        )]
    } else {
        app.session
            .available_models
            .iter()
            .map(|m| {
                let current_model = app
                    .config
                    .as_ref()
                    .map(|c| c.current_provider().model.as_str())
                    .unwrap_or("");
                let is_active = m == current_model;

                let prefix = if is_active { "★ " } else { "  " };
                let content = format!("{}{}", prefix, m);
                ListItem::new(content)
            })
            .collect()
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Available Models ")
                .borders(Borders::NONE),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol(">> ");

    let mut state = ListState::default().with_selected(Some(app.menu_state.selection_idx));
    frame.render_stateful_widget(list, area, &mut state);
} // render_models

fn render_providers(frame: &mut Frame, area: Rect, app: &App) {
    let empty = Vec::new();
    let providers = app.config.as_ref().map(|c| &c.providers).unwrap_or(&empty);
    let active_idx = app.config.as_ref().map(|c| c.active_provider).unwrap_or(0);

    let items: Vec<ListItem> = providers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let is_active = i == active_idx;
            let is_connected = app
                .agent
                .provider_connected
                .get(i)
                .copied()
                .unwrap_or(false);

            let status_dot = if is_connected {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::Red))
            };

            let prefix = if is_active { "✔ " } else { "  " };
            let name_style = if is_active {
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };

            let line = Line::from(vec![
                status_dot,
                Span::styled(format!("{}{}", prefix, p.name), name_style),
                Span::raw(format!(" ({})", p.model)),
                Span::styled(
                    format!(" - {}", p.base_url),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" API Providers ")
                .borders(Borders::NONE),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol(">> ");

    let mut state = ListState::default().with_selected(Some(app.menu_state.selection_idx));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(2)])
        .split(area);

    frame.render_stateful_widget(list, chunks[0], &mut state);

    let help_text = vec![
        Line::from(vec![
            Span::styled(" [a]", Style::default().fg(Color::Yellow)),
            Span::raw(" Add  "),
            Span::styled(" [d]", Style::default().fg(Color::Yellow)),
            Span::raw(" Delete  "),
            Span::styled(" [e]", Style::default().fg(Color::Yellow)),
            Span::raw(" Edit Key  "),
        ]),
        Line::from(vec![
            Span::styled(" [n]", Style::default().fg(Color::Yellow)),
            Span::raw(" Rename  "),
            Span::styled(" [u]", Style::default().fg(Color::Yellow)),
            Span::raw(" URL  "),
            Span::styled(" [m]", Style::default().fg(Color::Yellow)),
            Span::raw(" Model  "),
        ]),
    ];
    let help_para = Paragraph::new(help_text).block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(help_para, chunks[1]);
} // render_providers

fn render_skills(frame: &mut Frame, area: Rect, app: &App) {
    let mut items = Vec::new();

    // Local Skills
    if let Some(ref mgr) = app.manager {
        for skill in mgr.tool_registry().skills.lock().unwrap().iter() {
            let meta = skill.metadata();
            let line = Line::from(vec![
                Span::styled("📦 ", Style::default().fg(Color::Yellow)),
                Span::styled(meta.name.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(format!(" v{}", meta.version)),
                Span::styled(format!(" - {} tools", skill.tools().len()), Style::default().fg(Color::DarkGray)),
            ]);
            items.push(ListItem::new(line));
        }
    }

    // MCP Servers
    if let Some(ref cfg) = app.config {
        for mcp in &cfg.mcp_servers {
            let status_dot = if mcp.enabled {
                Span::styled("● ", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ ", Style::default().fg(Color::DarkGray))
            };

            let line = Line::from(vec![
                status_dot,
                Span::styled("MCP: ", Style::default().fg(Color::Cyan)),
                Span::styled(&mcp.name, Style::default().add_modifier(Modifier::BOLD)),
                if mcp.auto_install {
                    Span::styled(" [auto]", Style::default().fg(Color::Magenta))
                } else {
                    Span::raw("")
                },
                Span::styled(format!(" - {}", mcp.command), Style::default().fg(Color::DarkGray)),
            ]);
            items.push(ListItem::new(line));
        }
    }

    if items.is_empty() {
        items.push(ListItem::new(" No skills found."));
    }

    let list = List::new(items)
        .block(Block::default().title(" Agent Skills & MCP Servers ").borders(Borders::NONE))
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol(">> ");

    let mut state = ListState::default().with_selected(Some(app.menu_state.selection_idx));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_settings(frame: &mut Frame, area: Rect, app: &App) {
    let config = match app.config.as_ref() {
        Some(c) => c,
        None => return,
    };

    let settings = [
        format!("Working Directory: {}", config.agent.working_directory),
        format!("Max Iterations: {}", config.agent.max_iterations),
        format!("Auto-approve Tools: {}", config.agent.auto_approve_tools),
        format!("Theme: {}", config.ui.theme),
        format!("Show Reasoning: {}", config.ui.show_reasoning),
    ];

    let items: Vec<ListItem> = settings
        .iter()
        .map(|s| ListItem::new(format!(" {}", s)))
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Application Settings ")
                .borders(Borders::NONE),
        )
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .highlight_symbol(">> ");

    let mut state = ListState::default().with_selected(Some(app.menu_state.selection_idx));
    frame.render_stateful_widget(list, area, &mut state);
} // render_settings

fn render_help(frame: &mut Frame, area: Rect, _app: &App) {
    let text = vec![
        Line::from(""),
        Line::from(vec![Span::styled(
            " Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![
            Span::styled("   h / l / Tab", Style::default().fg(Color::Yellow)),
            Span::raw("   Switch between tabs"),
        ]),
        Line::from(vec![
            Span::styled("   j / k / ↑ / ↓", Style::default().fg(Color::Yellow)),
            Span::raw(" Select item in list"),
        ]),
        Line::from(vec![
            Span::styled("   Enter          ", Style::default().fg(Color::Yellow)),
            Span::raw(" Confirm selection"),
        ]),
        Line::from(vec![
            Span::styled("   d / Del        ", Style::default().fg(Color::Yellow)),
            Span::raw(" Delete item (Sessions)"),
        ]),
        Line::from(vec![
            Span::styled("   Esc / q        ", Style::default().fg(Color::Yellow)),
            Span::raw(" Close menu"),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            " Updates",
            Style::default().add_modifier(Modifier::BOLD),
        )]),
        Line::from(vec![Span::raw(
            " Selecting a model or provider will instantly reconfigure Seekr.",
        )]),
        Line::from(vec![Span::raw(
            " Settings changes apply to the current session and config file.",
        )]),
    ];

    let para = Paragraph::new(text)
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false });

    frame.render_widget(para, area);
} // render_help

fn render_footer(frame: &mut Frame, area: Rect, _app: &App) {
    let text = Line::from(vec![
        Span::styled(" [esc]", Style::default().fg(Color::Yellow)),
        Span::raw(" Close  "),
        Span::styled(" [Tab/h/l]", Style::default().fg(Color::Yellow)),
        Span::raw(" Tabs  "),
        Span::styled(" [j/k/Enter]", Style::default().fg(Color::Yellow)),
        Span::raw(" Select "),
    ]);

    let para = Paragraph::new(text).alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(para, area);
} // render_footer
