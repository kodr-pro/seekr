use crate::app::{LineType, VisualLine};
use crate::ui::syntax;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

/// Get per‑character styles for a visual line, taking syntax highlighting into account.
fn get_char_styles(vline: &VisualLine) -> Vec<Style> {
    let mut styles = Vec::new();

    // For code blocks, apply syntax highlighting
    if matches!(vline.line_type, LineType::CodeBlock) && !vline.text.is_empty() {
        let highlighted_spans = syntax::highlight_line(&vline.text, vline.language.as_deref());
        // Combine spans into per‑character styles
        for (style, chunk) in highlighted_spans {
            for _ in chunk.chars() {
                styles.push(style);
            }
        }
    } else {
        // Default style (will be overlaid by selection/cursor later)
        let default_style = if vline.is_header {
            match vline.text.as_str() {
                "[YOU]" => Style::default()
                    .fg(Color::Rgb(0, 255, 128))
                    .add_modifier(Modifier::BOLD),
                "[SEEKR]" => Style::default()
                    .fg(Color::Rgb(0, 191, 255))
                    .add_modifier(Modifier::BOLD),
                "[THINKING]" => Style::default()
                    .fg(Color::Rgb(255, 215, 0))
                    .add_modifier(Modifier::ITALIC | Modifier::DIM),
                "[ERROR]" => Style::default()
                    .fg(Color::Rgb(255, 69, 0))
                    .add_modifier(Modifier::BOLD),
                "[APPROVAL REQUIRED]" => Style::default()
                    .fg(Color::Rgb(255, 165, 0))
                    .add_modifier(Modifier::BOLD),
                "[INPUT REQUIRED]" => Style::default()
                    .fg(Color::Rgb(255, 255, 0))
                    .add_modifier(Modifier::BOLD),
                _ => Style::default()
                    .fg(Color::Rgb(200, 200, 200))
                    .add_modifier(Modifier::BOLD),
            }
        } else {
            Style::default().fg(Color::Rgb(220, 220, 220))
        };
        // Style copy icon differently for code block start lines
        let copy_icon = '⎘';
        for ch in vline.text.chars() {
            if vline.line_type == LineType::CodeBlockStart && ch == copy_icon {
                styles.push(
                    Style::default()
                        .fg(Color::Rgb(0, 255, 255))
                        .add_modifier(Modifier::BOLD),
                );
            } else {
                styles.push(default_style);
            }
        }
    }

    // Ensure we have exactly as many styles as characters
    // (highlight_line may produce fewer if there are zero‑width characters; we pad)
    let char_count = vline.text.chars().count();
    if styles.len() < char_count {
        let last_style = styles
            .last()
            .cloned()
            .unwrap_or(Style::default().fg(Color::White));
        styles.resize(char_count, last_style);
    } else if styles.len() > char_count {
        styles.truncate(char_count);
    }

    styles
}

pub fn render_chat(
    frame: &mut Frame,
    area: Rect,
    visual_lines: &[VisualLine],
    scroll_offset: u16,
    focused: bool,
) -> u16 {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Chat ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner = block.inner(area);
    if inner.width == 0 || inner.height == 0 {
        frame.render_widget(block, area);
        return 0;
    }

    let total_lines = visual_lines.len();
    let visible_height = inner.height;
    let max_scroll = total_lines.saturating_sub(visible_height as usize) as u16;
    let effective_scroll = scroll_offset.min(max_scroll) as usize;

    let mut lines_to_render = Vec::new();

    for vline in visual_lines
        .iter()
        .skip(effective_scroll)
        .take(visible_height as usize)
    {
        if vline.text.is_empty() {
            lines_to_render.push(Line::from(""));
            continue;
        }

        let char_styles = get_char_styles(vline);
        let chars: Vec<char> = vline.text.chars().collect();
        let mut spans = Vec::new();

        for (&c, &style) in chars.iter().zip(char_styles.iter()) {
            spans.push(Span::styled(c.to_string(), style));
        }

        lines_to_render.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines_to_render).block(block);

    frame.render_widget(paragraph, area);

    if max_scroll > 0 {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .symbols(ratatui::symbols::scrollbar::VERTICAL)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));

        let mut scrollbar_state = ScrollbarState::new(total_lines)
            .position(effective_scroll)
            .viewport_content_length(visible_height as usize);

        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin {
                vertical: 1,
                horizontal: 0,
            }),
            &mut scrollbar_state,
        );
    }

    max_scroll
} // render_chat
