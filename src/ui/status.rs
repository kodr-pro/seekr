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

    let separator = Span::styled(" • ", Style::default().fg(Color::Rgb(80, 80, 80)));

    let provider = Span::styled(
        info.provider.to_string(),
        Style::default()
            .fg(Color::Rgb(100, 149, 237)) // CornflowerBlue
            .add_modifier(Modifier::BOLD),
    );

    let model = Span::styled(
        info.model.to_string(),
        Style::default()
            .fg(Color::Rgb(0, 255, 255))
            .add_modifier(Modifier::BOLD),
    );

    let tokens = Span::styled(
        format!("{} tkn", format_tokens(info.total_tokens)),
        Style::default().fg(Color::Rgb(255, 215, 0)), // Gold
    );

    let iterations = Span::styled(
        format!("{}/{} loop", info.iteration, info.max_iterations),
        Style::default().fg(Color::Rgb(218, 112, 214)), // Orchid
    );

    let session = Span::styled(
        format!("ID: {}", info.session_id),
        Style::default()
            .fg(Color::Rgb(180, 180, 180))
            .add_modifier(Modifier::BOLD),
    );

    let thinking = if info.is_thinking {
        Span::styled(
            " ✨ FLOWING",
            Style::default()
                .fg(Color::Rgb(0, 255, 127)) // SpringGreen
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(" • IDLE", Style::default().fg(Color::Rgb(100, 100, 100)))
    };

    let line = Line::from(vec![
        Span::raw(" "),
        session,
        separator.clone(),
        conn_indicator,
        separator.clone(),
        provider,
        Span::raw(":"),
        model,
        separator.clone(),
        tokens,
        separator.clone(),
        iterations,
        separator,
        thinking,
    ]);

    let paragraph =
        Paragraph::new(line).style(Style::default().bg(Color::Rgb(20, 20, 20)).fg(Color::White));

    frame.render_widget(paragraph, area);
}

fn format_tokens(tokens: u32) -> String {
    if tokens >= 1000 {
        format!("{:.1}k", tokens as f64 / 1000.0)
    } else {
        tokens.to_string()
    }
}
