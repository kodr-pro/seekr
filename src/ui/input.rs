use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

pub fn render_input(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    cursor_pos: usize,
    active: bool,
    prompt: Option<&str>,
    shell_context: Option<&str>,
) {
    let is_shell = prompt.is_some();

    let (context_area, input_area) = if is_shell && shell_context.is_some() {
        let context_lines = shell_context.unwrap().lines().count().max(1) as u16;
        let context_height = (context_lines + 2).min(area.height.saturating_sub(4));
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(context_height),
                Constraint::Min(3),
            ])
            .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    if let (Some(ctx_area), Some(ctx_text)) = (context_area, shell_context) {
        let ctx_block = Block::default()
            .title(" Process output ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let ctx_para = Paragraph::new(ctx_text)
            .block(ctx_block)
            .wrap(ratatui::widgets::Wrap { trim: true })
            .style(Style::default().fg(Color::DarkGray));
        frame.render_widget(ctx_para, ctx_area);
    }

    let border_color = if is_shell {
        Color::Rgb(255, 165, 0)
    } else if active {
        Color::Cyan
    } else {
        Color::DarkGray
    };

    let title = if let Some(p) = prompt {
        format!(" ⌨ {} ", p)
    } else {
        " > Type your message (Enter=Send, Tab=Focus, Ctrl+g=Menu, Esc=Quit) ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let display_text = if input.is_empty() && !active && !is_shell {
        Line::from(Span::styled(
            "Type your message...",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        ))
    } else {
        let chars: Vec<char> = input.chars().collect();
        let before: String = chars.iter().take(cursor_pos).collect();
        let cursor_char = chars.get(cursor_pos).map(|c| c.to_string()).unwrap_or_else(|| " ".to_string());
        let after: String = chars.iter().skip(cursor_pos + 1).collect();

        Line::from(vec![
            Span::styled(before, Style::default().fg(Color::White)),
            Span::styled(
                cursor_char,
                Style::default()
                    .fg(Color::Black)
                    .bg(if is_shell { Color::Rgb(255, 165, 0) } else { Color::White }),
            ),
            Span::styled(after, Style::default().fg(Color::White)),
        ])
    };

    let paragraph = Paragraph::new(display_text)
        .block(block)
        .wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(paragraph, input_area);
} // render_input
