// ui/chat.rs - Chat panel rendering
//
// Renders the scrollable chat history with user/assistant messages,
// tool calls shown inline, and reasoning tokens in dimmed text.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::ChatEntry;

/// Render the chat panel
pub fn render_chat(frame: &mut Frame, area: Rect, entries: &[ChatEntry], scroll_offset: u16, focused: bool) {
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

    // Build lines from chat entries
    let mut lines: Vec<Line> = Vec::new();

    for entry in entries {
        match entry {
            ChatEntry::UserMessage(msg) => {
                lines.push(Line::from(vec![
                    Span::styled("You: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::styled(msg.as_str(), Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from(""));
            }
            ChatEntry::AssistantContent(text) => {
                // Wrap long assistant messages
                for line_str in text.lines() {
                    if !line_str.trim().is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled(line_str, Style::default().fg(Color::White)),
                        ]));
                    }
                }
                lines.push(Line::from(""));
            }
            ChatEntry::AssistantStreaming(text) => {
                if !text.is_empty() {
                    for line_str in text.lines() {
                        if !line_str.trim().is_empty() {
                            lines.push(Line::from(vec![
                                Span::styled(line_str, Style::default().fg(Color::White)),
                            ]));
                        }
                    }
                }
                lines.push(Line::from(vec![
                    Span::styled("...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
                ]));
            }
            ChatEntry::Reasoning(text) => {
                lines.push(Line::from(vec![
                    Span::styled("[thinking] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM)),
                ]));
                for line_str in text.lines() {
                    if !line_str.trim().is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled(line_str, Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
                        ]));
                    }
                }
                lines.push(Line::from(""));
            }
            ChatEntry::ToolCall { name, arguments } => {
                let args_short = if arguments.len() > 60 {
                    format!("{}...", &arguments[..60])
                } else {
                    arguments.clone()
                };
                lines.push(Line::from(vec![
                    Span::styled("> ", Style::default().fg(Color::Magenta)),
                    Span::styled(name.as_str(), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("({})", args_short), Style::default().fg(Color::DarkGray)),
                ]));
            }
            ChatEntry::ToolResult { name, result } => {
                let result_short = if result.len() > 200 {
                    format!("{}...", &result[..200])
                } else {
                    result.clone()
                };
                lines.push(Line::from(vec![
                    Span::styled("  = ", Style::default().fg(Color::Blue)),
                    Span::styled(format!("[{}] ", name), Style::default().fg(Color::Blue)),
                    Span::styled(result_short, Style::default().fg(Color::DarkGray)),
                ]));
                lines.push(Line::from(""));
            }
            ChatEntry::Error(msg) => {
                lines.push(Line::from(vec![
                    Span::styled("Error: ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::styled(msg.as_str(), Style::default().fg(Color::Red)),
                ]));
                lines.push(Line::from(""));
            }
            ChatEntry::SystemInfo(msg) => {
                lines.push(Line::from(vec![
                    Span::styled(msg.as_str(), Style::default().fg(Color::Yellow).add_modifier(Modifier::DIM)),
                ]));
                lines.push(Line::from(""));
            }
            ChatEntry::ToolApproval { name, arguments } => {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("Agent wants to execute: {}({})", name, if arguments.len() > 40 { format!("{}...", &arguments[..40]) } else { arguments.clone() }),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled(
                        "  [Y]es / [N]o / [A]lways",
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
            ChatEntry::CliInputPrompt(prompt) => {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("CLI Input Required: {}", prompt),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled(
                        "  Type your response in the input bar below.",
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
            }
        }
    }

    // Calculate scroll
    let total_lines = lines.len() as u16;
    let visible_height = inner.height;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let effective_scroll = scroll_offset.min(max_scroll);

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll, 0));

    frame.render_widget(paragraph, area);
}
