use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::SetupState;

const LOGO: &str = r#"
  ____  _____ _____ _  ______
 / ___|| ____| ____| |/ /  _ \
 \___ \|  _| |  _| | ' /| |_) |
  ___) | |___| |___| . \|  _ <
 |____/|_____|_____|_|\_\_| \_\
"#;

pub fn render_setup(frame: &mut Frame, area: Rect, state: &SetupState) {
    frame.render_widget(Clear, area);

    let layout = crate::ui::layout::SetupLayout::new(area);

    render_logo(frame, layout.header);

    match state.current_step {
        0 => render_welcome(frame, layout.content),
        1 => render_provider_step(frame, layout.content, state),
        2 => render_api_key_step(frame, layout.content, state),
        3 => render_model_step(frame, layout.content, state),
        4 => render_auto_approve_step(frame, layout.content, state),
        5 => render_working_dir_step(frame, layout.content, state),
        6 => render_validating_step(frame, layout.content, state),
        7 => render_complete_step(frame, layout.content, state),
        _ => {}
    }

    render_footer(frame, layout.footer, state);
} // render_setup

fn render_logo(frame: &mut Frame, area: Rect) {
    let logo = Paragraph::new(LOGO)
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);
    frame.render_widget(logo, area);
} // render_logo

fn render_welcome(frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Welcome to Seekr!",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "An AI agent manager for your terminal, compatible with OpenAI-style APIs.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "This wizard will help you set up your configuration.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to continue...",
            Style::default().fg(Color::Yellow),
        )),
    ];

    let block = Block::default()
        .title(" Setup Wizard ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(text)
        .block(block)
        .alignment(Alignment::Center);
    frame.render_widget(paragraph, area);
} // render_welcome

fn render_provider_step(frame: &mut Frame, area: Rect, state: &SetupState) {
    let providers = ["OpenAI", "DeepSeek", "Anthropic", "Custom / Other"];
    let text: Vec<Line> = std::iter::once(Line::from(""))
        .chain(std::iter::once(Line::from(Span::styled(
            "Step 1: AI Provider",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))))
        .chain(std::iter::once(Line::from("")))
        .chain(std::iter::once(Line::from(Span::styled(
            "Select your AI provider:",
            Style::default().fg(Color::Gray),
        ))))
        .chain(std::iter::once(Line::from("")))
        .chain(providers.iter().enumerate().map(|(i, provider)| {
            let selected = i == state.provider_selection;
            let prefix = if selected { " > " } else { "   " };
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(format!("{}{}", prefix, provider), style))
        }))
        .collect();

    render_step_content(frame, area, text);
} // render_provider_step

fn render_api_key_step(frame: &mut Frame, area: Rect, state: &SetupState) {
    let masked = "*".repeat(state.api_key_input.len());
    let display = if state.api_key_input.is_empty() {
        "Enter your API key...".to_string()
    } else {
        masked
    };

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Step 2: API Key",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter your provider API key (e.g. OpenAI, DeepSeek, etc.):",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  > Keys are securely stored in the OS Keyring and NOT in config.toml",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(Span::styled(
            "  > For headless usage, set SEEKR_API_KEY=<key> or SEEKR_API_KEY_OPENAI=<key>",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  > {}", display),
            if state.api_key_input.is_empty() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::Green)
            },
        )),
    ];

    if let Some(ref err) = state.error_message {
        let mut t = text;
        t.push(Line::from(""));
        t.push(Line::from(Span::styled(
            format!("  Error: {}", err),
            Style::default().fg(Color::Red),
        )));
        render_step_content(frame, area, t);
    } else {
        render_step_content(frame, area, text);
    }
} // render_api_key_step

fn render_model_step(frame: &mut Frame, area: Rect, state: &SetupState) {
    let models = match state.provider_selection {
        0 => vec!["gpt-4o", "gpt-4o-mini"],
        1 => vec!["deepseek-chat", "deepseek-reasoner"],
        2 => vec!["claude-3-5-sonnet-latest"],
        _ => vec![
            "gpt-4o",
            "gpt-4o-mini",
            "claude-3-5-sonnet-latest",
            "deepseek-chat",
            "deepseek-reasoner",
        ],
    };

    let text: Vec<Line> = std::iter::once(Line::from(""))
        .chain(std::iter::once(Line::from(Span::styled(
            "Step 3: Default Model",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))))
        .chain(std::iter::once(Line::from("")))
        .chain(std::iter::once(Line::from(Span::styled(
            "Select the default model:",
            Style::default().fg(Color::Gray),
        ))))
        .chain(std::iter::once(Line::from("")))
        .chain(models.iter().enumerate().map(|(i, model)| {
            let selected = i == state.model_selection;
            let prefix = if selected { " > " } else { "   " };
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(format!("{}{}", prefix, model), style))
        }))
        .collect();

    render_step_content(frame, area, text);
} // render_model_step

fn render_auto_approve_step(frame: &mut Frame, area: Rect, state: &SetupState) {
    let options = [
        "No (recommended - ask before each tool execution)",
        "Yes (auto-approve all tool executions)",
    ];
    let text: Vec<Line> = std::iter::once(Line::from(""))
        .chain(std::iter::once(Line::from(Span::styled(
            "Step 4: Auto-approve Tools",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ))))
        .chain(std::iter::once(Line::from("")))
        .chain(std::iter::once(Line::from(Span::styled(
            "Should the agent automatically execute tools without confirmation?",
            Style::default().fg(Color::Gray),
        ))))
        .chain(std::iter::once(Line::from("")))
        .chain(options.iter().enumerate().map(|(i, opt)| {
            let selected = i == state.auto_approve_selection;
            let prefix = if selected { " > " } else { "   " };
            let style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(format!("{}{}", prefix, opt), style))
        }))
        .collect();

    render_step_content(frame, area, text);
} // render_auto_approve_step

fn render_working_dir_step(frame: &mut Frame, area: Rect, state: &SetupState) {
    let display = if state.working_dir_input.is_empty() {
        ".".to_string()
    } else {
        state.working_dir_input.clone()
    };

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Step 5: Working Directory",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter the default working directory:",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("  > {}", display),
            Style::default().fg(Color::Green),
        )),
    ];

    render_step_content(frame, area, text);
} // render_working_dir_step

fn render_validating_step(frame: &mut Frame, area: Rect, state: &SetupState) {
    let mut text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Validating Configuration...",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Testing API key...",
            Style::default().fg(Color::Yellow),
        )),
    ];

    if let Some(ref err) = state.error_message {
        text.push(Line::from(""));
        text.push(Line::from(Span::styled(
            format!("Error: {}", err),
            Style::default().fg(Color::Red),
        )));
        text.push(Line::from(""));
        text.push(Line::from(Span::styled(
            "Press Enter to go back...",
            Style::default().fg(Color::Yellow),
        )));
    }

    render_step_content(frame, area, text);
} // render_validating_step

fn render_complete_step(frame: &mut Frame, area: Rect, _state: &SetupState) {
    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Setup Complete!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Your configuration has been saved.",
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to start Seekr...",
            Style::default().fg(Color::Yellow),
        )),
    ];

    render_step_content(frame, area, text);
} // render_complete_step

fn render_step_content(frame: &mut Frame, area: Rect, lines: Vec<Line>) {
    let block = Block::default()
        .title(" Setup Wizard ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
} // render_step_content

fn render_footer(frame: &mut Frame, area: Rect, state: &SetupState) {
    let nav = match state.current_step {
        0 => "Enter: Continue",
        1 | 4 | 5 => "Up/Down: Select | Enter: Next | Esc: Back",
        2 => "Enter: Next | Esc: Back",
        3 => "Up/Down: Select | Enter: Next | Esc: Back",
        6 => {
            if state.error_message.is_some() {
                "Enter: Go back"
            } else {
                "Validating..."
            }
        }
        7 => "Enter: Start",
        _ => "",
    };

    let step_text = if state.current_step > 0 && state.current_step < 6 {
        format!("Step {}/5", state.current_step)
    } else {
        String::new()
    };

    let line = Line::from(vec![
        Span::styled(format!(" {} ", nav), Style::default().fg(Color::DarkGray)),
        Span::raw("  "),
        Span::styled(step_text, Style::default().fg(Color::DarkGray)),
    ]);

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
} // render_footer
