use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use crate::app::{ChatSelection, SelectionMode, VisualLine, LineType};
use crate::ui::syntax;
use std::cmp::min;

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
                "[YOU]" => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                "[SEEKR]" => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                "[THINKING]" => Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC | Modifier::DIM),
                "[ERROR]" => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                "[APPROVAL REQUIRED]" => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                "[INPUT REQUIRED]" => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                _ => Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            }
        } else {
            Style::default().fg(Color::White)
        };
        // Style copy icon differently for code block start lines
        let copy_icon = '⎘';
        for ch in vline.text.chars() {
            if vline.line_type == LineType::CodeBlockStart && ch == copy_icon {
                styles.push(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
            } else {
                styles.push(default_style);
            }
        }
    }
    
    // Ensure we have exactly as many styles as characters
    // (highlight_line may produce fewer if there are zero‑width characters; we pad)
    let char_count = vline.text.chars().count();
    if styles.len() < char_count {
        let last_style = styles.last().cloned().unwrap_or(Style::default().fg(Color::White));
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
    selection: &ChatSelection,
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
    
    // Determine selection range
    let (sel_start_v, sel_start_c, sel_end_v, sel_end_c) = if let Some(av) = selection.anchor_vline {
        let ac = selection.anchor_col.unwrap_or(0);
        let (s_v, s_c, e_v, e_c) = if (av, ac) <= (selection.vline, selection.col) {
            (av, ac, selection.vline, selection.col)
        } else {
            (selection.vline, selection.col, av, ac)
        };
        
        if selection.mode == SelectionMode::VisualLine {
            (s_v, 0, e_v, visual_lines.get(e_v).map(|l| l.text.chars().count()).unwrap_or(0))
        } else {
            (s_v, s_c, e_v, e_c)
        }
    } else {
        (0, 0, 0, 0)
    };

    let has_selection = selection.mode != SelectionMode::Normal && selection.anchor_vline.is_some();

    for vidx in effective_scroll..min(effective_scroll + visible_height as usize, total_lines) {
        let vline = &visual_lines[vidx];
        
        if vline.text.is_empty() {
            lines_to_render.push(Line::from(""));
            continue;
        }

        let char_styles = get_char_styles(vline);
        let chars: Vec<char> = vline.text.chars().collect();
        let mut spans = Vec::new();
        
        for (cidx, (&c, &style)) in chars.iter().zip(char_styles.iter()).enumerate() {
            let mut char_style = style;
            
            // Apply selection highlight
            let in_selection = has_selection && (
                (vidx > sel_start_v && vidx < sel_end_v) ||
                (vidx == sel_start_v && vidx == sel_end_v && cidx >= sel_start_c && cidx <= sel_end_c) ||
                (vidx == sel_start_v && vidx < sel_end_v && cidx >= sel_start_c) ||
                (vidx == sel_end_v && vidx > sel_start_v && cidx <= sel_end_c)
            );

            if in_selection {
                char_style = char_style.bg(Color::Rgb(60, 60, 100));
            }

            // Apply cursor highlight
            if focused && vidx == selection.vline && cidx == selection.col {
                char_style = char_style.bg(Color::White).fg(Color::Black).remove_modifier(Modifier::DIM);
            }
            
            spans.push(Span::styled(c.to_string(), char_style));
        }

        // Handle cursor at end of line
        if focused && vidx == selection.vline && selection.col >= chars.len() {
            spans.push(Span::styled(" ", Style::default().bg(Color::White).fg(Color::Black)));
        }

        lines_to_render.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines_to_render)
        .block(block);

    frame.render_widget(paragraph, area);

    if max_scroll > 0 {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .symbols(ratatui::symbols::scrollbar::VERTICAL)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        
        let mut scrollbar_state = ScrollbarState::new(total_lines as usize)
            .position(effective_scroll as usize)
            .viewport_content_length(visible_height as usize);
            
        frame.render_stateful_widget(
            scrollbar,
            area.inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
            &mut scrollbar_state,
        );
    }

    max_scroll
} // render_chat