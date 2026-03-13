// ui/input.rs - Input bar rendering
//
// Renders the text input area at the bottom of the screen.
// Shows the current input text with a cursor indicator.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

/// Render the input bar
pub fn render_input(frame: &mut Frame, area: Rect, input: &str, cursor_pos: usize, active: bool, prompt: Option<&str>) {
    let border_style = if prompt.is_some() {
        Style::default().fg(Color::Yellow)
    } else if active {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let title = if let Some(p) = prompt {
        format!(" [PROMPT] {} ", p)
    } else {
        " > Type your message (Enter=Send, Esc=Quit, Tab=Focus) ".to_string()
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let display_text = if input.is_empty() && !active {
        Line::from(Span::styled(
            "Type your message...",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        ))
    } else {
        // Show the input with cursor position
        let before = &input[..cursor_pos.min(input.len())];
        let cursor_char = input.get(cursor_pos..cursor_pos + 1).unwrap_or(" ");
        let after_start = (cursor_pos + 1).min(input.len());
        let after = &input[after_start..];

        Line::from(vec![
            Span::styled(before, Style::default().fg(Color::White)),
            Span::styled(
                cursor_char,
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White),
            ),
            Span::styled(after, Style::default().fg(Color::White)),
        ])
    };

    let paragraph = Paragraph::new(display_text)
        .block(block)
        .wrap(ratatui::widgets::Wrap { trim: true });
    frame.render_widget(paragraph, area);
}
