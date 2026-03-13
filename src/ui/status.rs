// ui/status.rs - Status bar rendering
//
// Shows connection status, model name, token count, and iteration count
// at the bottom of the application.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

/// Status information for the bar
pub struct StatusInfo<'a> {
    pub session_id: &'a str,
    pub connected: bool,
    pub model: &'a str,
    pub total_tokens: u32,
    pub iteration: u32,
    pub max_iterations: u32,
}

/// Render the status bar
pub fn render_status(frame: &mut Frame, area: Rect, info: &StatusInfo) {
    let conn_indicator = if info.connected {
        Span::styled("● Connected", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ Disconnected", Style::default().fg(Color::Red))
    };

    let separator = Span::styled(" │ ", Style::default().fg(Color::DarkGray));

    let model = Span::styled(
        format!("Model: {}", info.model),
        Style::default().fg(Color::Cyan),
    );

    let tokens = Span::styled(
        format!("Tokens: {}", format_tokens(info.total_tokens)),
        Style::default().fg(Color::Yellow),
    );

    let iterations = Span::styled(
        format!("{}/{} iter", info.iteration, info.max_iterations),
        Style::default().fg(Color::Magenta).add_modifier(Modifier::DIM),
    );

    let session = Span::styled(
        format!("Session: {}", info.session_id),
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
    );

    let line = Line::from(vec![
        Span::raw(" "),
        session,
        separator.clone(),
        conn_indicator,
        separator.clone(),
        model,
        separator.clone(),
        tokens,
        separator,
        iterations,
    ]);

    let paragraph = Paragraph::new(line)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    frame.render_widget(paragraph, area);
}

/// Format token count with K suffix for readability
fn format_tokens(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}
