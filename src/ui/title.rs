// ui/title.rs - Title bar rendering
//
// Renders the application title bar with version information
// and current session/connection status.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

/// Title information for the bar
pub struct TitleInfo<'a> {
    pub version: &'a str,
    pub session_id: Option<&'a str>,
    pub connected: bool,
    pub model: &'a str,
}

/// Render the title bar
pub fn render_title(frame: &mut Frame, area: Rect, info: &TitleInfo) {
    // Build the title line
    let title = Span::styled(
        "SEEKR",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let version = Span::styled(
        format!(" v{}", info.version),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    );

    let separator = Span::styled(" │ ", Style::default().fg(Color::DarkGray));

    let model = Span::styled(
        info.model,
        Style::default().fg(Color::Yellow),
    );

    let session_info = if let Some(sid) = info.session_id {
        Span::styled(
            format!("Session: {}", sid),
            Style::default().fg(Color::Magenta),
        )
    } else {
        Span::styled(
            "New Session",
            Style::default().fg(Color::DarkGray),
        )
    };

    let conn_status = if info.connected {
        Span::styled(
            "● Connected",
            Style::default().fg(Color::Green),
        )
    } else {
        Span::styled(
            "○ Disconnected",
            Style::default().fg(Color::Red),
        )
    };

    let line = Line::from(vec![
        Span::raw(" "),
        title,
        version,
        separator.clone(),
        model,
        separator.clone(),
        session_info,
        Span::raw(" "),
        Span::styled("│", Style::default().fg(Color::DarkGray)),
        Span::raw(" "),
        conn_status,
        Span::raw(" "),
    ]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray));

    let paragraph = Paragraph::new(line)
        .block(block)
        .style(Style::default().bg(Color::Black).fg(Color::White));

    frame.render_widget(paragraph, area);
}