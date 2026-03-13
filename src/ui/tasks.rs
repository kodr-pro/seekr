// ui/tasks.rs - Task and activity panel rendering
//
// Renders the task list with status indicators and the activity log
// of recent tool executions.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::tools::task::{Task, TaskStatus};
use crate::tools::ActivityEntry;

/// Render the right-side task + activity panel
pub fn render_tasks(
    frame: &mut Frame,
    area: Rect,
    tasks: &[Task],
    activities: &[ActivityEntry],
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Split vertically: tasks (top half) | activity log (bottom half)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(area);

    // Task list
    render_task_list(frame, chunks[0], tasks, border_style);

    // Activity log
    render_activity_log(frame, chunks[1], activities, border_style);
}

fn render_task_list(frame: &mut Frame, area: Rect, tasks: &[Task], border_style: Style) {
    let block = Block::default()
        .title(" Tasks ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let mut lines: Vec<Line> = Vec::new();

    if tasks.is_empty() {
        lines.push(Line::from(Span::styled(
            "No tasks yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        for task in tasks {
            let color = match task.status {
                TaskStatus::Pending => Color::DarkGray,
                TaskStatus::InProgress => Color::Yellow,
                TaskStatus::Completed => Color::Green,
                TaskStatus::Failed => Color::Red,
            };
            let icon = task.status.icon();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", icon),
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!("[{}] ", task.status),
                    Style::default().fg(color).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    task.title.as_str(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}

fn render_activity_log(frame: &mut Frame, area: Rect, activities: &[ActivityEntry], border_style: Style) {
    let block = Block::default()
        .title(" Activity ")
        .borders(Borders::ALL)
        .border_style(border_style);

    let mut lines: Vec<Line> = Vec::new();

    if activities.is_empty() {
        lines.push(Line::from(Span::styled(
            "No activity yet",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        // Show the most recent activities (last N that fit)
        let start = activities.len().saturating_sub(20);
        for activity in &activities[start..] {
            lines.push(Line::from(Span::styled(
                activity.summary.as_str(),
                Style::default().fg(Color::Cyan),
            )));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
}
