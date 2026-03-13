// ui/chat.rs - Chat panel rendering
//
// Renders the scrollable chat history with user/assistant messages,
// tool calls shown inline, and reasoning tokens in dimmed text.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::app::ChatEntry;

/// Render the chat panel
pub fn render_chat(frame: &mut Frame, area: Rect, entries: &[ChatEntry], scroll_offset: u16, focused: bool) -> u16 {
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

    // Build the full text content
    let mut all_text = Text::default();

    for (idx, entry) in entries.iter().enumerate() {
        if idx > 0 {
            all_text.push_line(Line::from(""));
        }

        match entry {
            ChatEntry::UserMessage(msg) => {
                all_text.push_line(Line::from(vec![
                    Span::styled("● You", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                ]));
                all_text.push_line(Line::from(vec![
                    Span::styled(msg.as_str(), Style::default().fg(Color::White)),
                ]));
            }
            ChatEntry::AssistantContent(text) | ChatEntry::AssistantStreaming(text) => {
                let is_streaming = matches!(entry, ChatEntry::AssistantStreaming(_));
                all_text.push_line(Line::from(vec![
                    Span::styled("● Assistant", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                ]));
                
                // Restore structure-preserving rendering but avoid excessive empty lines
                let processed = if text.is_empty() && is_streaming { "..." } else { text.as_str() };
                for line in processed.lines() {
                    all_text.push_line(Line::from(vec![
                        Span::styled(line, Style::default().fg(Color::White)),
                    ]));
                }

                if is_streaming && !text.ends_with("...") {
                     if let Some(last_line) = all_text.lines.last_mut() {
                        last_line.spans.push(Span::styled(" ▂", Style::default().fg(Color::Cyan).add_modifier(Modifier::SLOW_BLINK)));
                     }
                }
            }
            ChatEntry::Reasoning(text) => {
                all_text.push_line(Line::from(vec![
                    Span::styled("◌ Thinking", Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC | Modifier::DIM)),
                ]));
                for line in text.lines() {
                    if !line.trim().is_empty() {
                        all_text.push_line(Line::from(vec![
                            Span::styled(line, Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
                        ]));
                    }
                }
            }
            ChatEntry::ToolCall { name, arguments } => {
                let args_short = if arguments.len() > 64 {
                    format!("{}...", &arguments[..64])
                } else {
                    arguments.clone()
                };
                all_text.push_line(Line::from(vec![
                    Span::styled("➞ Tool Call: ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                    Span::styled(name.as_str(), Style::default().fg(Color::Magenta)),
                    Span::styled(format!(" ({})", args_short), Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                ]));
            }
            ChatEntry::ToolResult { name, result } => {
                all_text.push_line(Line::from(vec![
                    Span::styled("✓ Tool Result: ", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                    Span::styled(name.as_str(), Style::default().fg(Color::Blue)),
                ]));

                let max_len = 2000;
                let display_result = if result.len() > max_len {
                    format!("{}... (truncated)", &result[..max_len])
                } else {
                    result.clone()
                };

                for line in display_result.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        all_text.push_line(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled(trimmed.to_string(), Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                }
            }
            ChatEntry::Error(msg) => {
                all_text.push_line(Line::from(vec![
                    Span::styled("✖ Error", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                ]));
                all_text.push_line(Line::from(vec![
                    Span::styled(msg.as_str(), Style::default().fg(Color::Red)),
                ]));
            }
            ChatEntry::SystemInfo(msg) => {
                all_text.push_line(Line::from(vec![
                    Span::styled(format!("ℹ {}", msg), Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM)),
                ]));
            }
            ChatEntry::ToolApproval { name, arguments } => {
                all_text.push_line(Line::from(vec![
                    Span::styled("‼ Approval Required", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                ]));
                all_text.push_line(Line::from(vec![
                    Span::styled(format!("Agent wants to execute: {}({})", name, arguments), Style::default().fg(Color::White)),
                ]));
                all_text.push_line(Line::from(vec![
                    Span::styled("  [Y]es / [N]o / [A]lways", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                ]));
            }
            ChatEntry::CliInputPrompt(prompt) => {
                all_text.push_line(Line::from(vec![
                    Span::styled("⌨ Input Required", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                ]));
                all_text.push_line(Line::from(vec![
                    Span::styled(prompt.as_str(), Style::default().fg(Color::White)),
                ]));
            }
        }
    }

    // Now wrap and calculate height accurately
    let wrap_width = inner.width;
    let mut total_lines = 0;
    for line in all_text.lines.iter() {
        let line_width = line.width();
        if line_width == 0 {
            total_lines += 1;
        } else {
            total_lines += (line_width as u16 + wrap_width - 1).saturating_div(wrap_width.max(1));
        }
    }

    let visible_height = inner.height;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = scroll_offset.min(max_scroll);

    let paragraph = Paragraph::new(all_text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll, 0));

    frame.render_widget(paragraph, area);

    // Render scrollbar
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
}
