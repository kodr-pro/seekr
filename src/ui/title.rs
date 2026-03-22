use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub struct TitleInfo<'a> {
    pub version: &'a str,
    pub new_version: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub connected: bool,
    pub model: &'a str,
    pub status: &'a str,
}

pub fn render_title(frame: &mut Frame, area: Rect, info: &TitleInfo) {
    let title = Span::styled(
        " SEEKR ",
        Style::default()
            .bg(Color::Rgb(0, 191, 255))
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD),
    );

    let version = Span::styled(
        format!(" v{}", info.version),
        Style::default().fg(Color::Rgb(150, 150, 150)),
    );

    let separator = Span::styled("  ", Style::default());

    let model = Span::styled(
        format!(" MODEL: {}", info.model),
        Style::default()
            .fg(Color::Rgb(255, 215, 0))
            .add_modifier(Modifier::BOLD),
    );

    let session_info = if let Some(sid) = info.session_id {
        Span::styled(
            format!(" SESSION: {}", sid),
            Style::default().fg(Color::Rgb(186, 85, 211)),
        )
    } else {
        Span::styled(
            " SESSION: NEW",
            Style::default().fg(Color::Rgb(100, 100, 100)),
        )
    };

    let conn_status = if info.connected {
        Span::styled(" ● ONLINE", Style::default().fg(Color::Rgb(0, 255, 127)))
    } else {
        Span::styled(" ○ OFFLINE", Style::default().fg(Color::Rgb(255, 69, 0)))
    };

    let status_color = if info.status.to_lowercase() == "ready" {
        Color::Rgb(0, 255, 127)
    } else {
        Color::Rgb(255, 215, 0)
    };
    let status = Span::styled(
        format!(" {}", info.status.to_uppercase()),
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    );

    let mut spans = vec![title, version];

    if let Some(nv) = info.new_version {
        spans.push(separator.clone());
        spans.push(Span::styled(
            format!(" 🎁 UPDATE: v{} ", nv),
            Style::default()
                .bg(Color::Rgb(255, 69, 0))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
    }

    spans.extend(vec![
        separator.clone(),
        status,
        separator.clone(),
        model,
        separator.clone(),
        session_info,
        separator,
        conn_status,
    ]);

    let line = Line::from(spans);

    let block = Block::default().borders(Borders::NONE);

    let paragraph = Paragraph::new(line)
        .block(block)
        .style(Style::default().bg(Color::Rgb(30, 30, 30)).fg(Color::White));

    frame.render_widget(paragraph, area);
} // render_title
