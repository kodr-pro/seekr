use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

pub struct StatusInfo<'a> {
    pub session_id: &'a str,
    pub connected: bool,
    pub provider: &'a str,
    pub model: &'a str,
    pub total_tokens: u32,
    pub iteration: u32,
    pub max_iterations: u32,
    pub is_thinking: bool,
}

pub fn render_status(frame: &mut Frame, area: Rect, info: &StatusInfo) {
    let conn_indicator = if info.connected {
        Span::styled("● Connected", Style::default().fg(Color::Green))
    } else {
        Span::styled("○ Disconnected", Style::default().fg(Color::Red))
    };

    let separator = Span::styled(" │ ", Style::default().fg(Color::DarkGray));

    let provider = Span::styled(
        format!("Provider: {}", info.provider),
        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
    );

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

    let thinking = if info.is_thinking {
        Span::styled(" Thinking...", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" Idle", Style::default().fg(Color::DarkGray))
    };

    let line = Line::from(vec![
        Span::raw(" "),
        session,
        separator.clone(),
        conn_indicator,
        separator.clone(),
        provider,
        separator.clone(),
        model,
        separator.clone(),
        tokens,
        separator.clone(),
        iterations,
        separator,
        thinking,
    ]);

    let paragraph = Paragraph::new(line)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White));

    frame.render_widget(paragraph, area);
} // render_status

fn format_tokens(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
} // format_tokens
