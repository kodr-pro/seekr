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
                // Render all lines including blank lines for proper markdown structure.
                // Split on '\n' instead of .lines() to preserve trailing partial lines.
                let raw_lines: Vec<&str> = text.split('\n').collect();
                for (i, line_str) in raw_lines.iter().enumerate() {
                    if line_str.trim().is_empty() {
                        // Preserve blank lines (paragraph spacing, list separation)
                        // but skip a trailing blank at the very end to avoid double-spacing.
                        if i < raw_lines.len() - 1 {
                            lines.push(Line::from(""));
                        }
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled(*line_str, Style::default().fg(Color::White)),
                        ]));
                    }
                }
                lines.push(Line::from(""));
            }
            ChatEntry::AssistantStreaming(text) => {
                if !text.is_empty() {
                    // Use split('\n') so a trailing partial line (no newline yet) is not lost.
                    let raw_lines: Vec<&str> = text.split('\n').collect();
                    for (i, line_str) in raw_lines.iter().enumerate() {
                        if line_str.trim().is_empty() {
                            if i < raw_lines.len() - 1 {
                                lines.push(Line::from(""));
                            }
                        } else {
                            lines.push(Line::from(vec![
                                Span::styled(*line_str, Style::default().fg(Color::White)),
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
                for line_str in text.split('\n') {
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
                lines.push(Line::from(vec![
                    Span::styled("  = ", Style::default().fg(Color::Blue)),
                    Span::styled(format!("[{}]", name), Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
                ]));

                let lines_to_show = if result.len() > 1000 {
                    let mut s = result.chars().take(1000).collect::<String>();
                    s.push_str("...");
                    s
                } else {
                    result.clone()
                };

                for line in lines_to_show.split('\n') {
                    if line.starts_with('+') {
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(line.to_string(), Style::default().fg(Color::Green)),
                        ]));
                    } else if line.starts_with('-') {
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(line.to_string(), Style::default().fg(Color::Red)),
                        ]));
                    } else if line.starts_with("@@") {
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(line.to_string(), Style::default().fg(Color::Cyan)),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled("    ", Style::default()),
                            Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
                        ]));
                    }
                }
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

    // Calculate scroll: clamp so we never scroll past the last line
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
