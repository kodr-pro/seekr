use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap, Gauge},
};

use crate::tools::task::{Task, TaskStatus};
use crate::tools::ActivityEntry;

pub fn render_tasks(
    frame: &mut Frame,
    area: Rect,
    tasks: &[Task],
    activities: &[ActivityEntry],
    live_activities: &[ActivityEntry],
    focused: bool,
) {
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(if live_activities.is_empty() { 0 } else { (live_activities.len() as u16 * 2) + 2 }),
            Constraint::Percentage(40),
        ])
        .split(area);

    render_task_list(frame, chunks[0], tasks, activities, border_style);
    if !live_activities.is_empty() {
        render_active_threads(frame, chunks[1], live_activities, border_style);
    }
    render_activity_log(frame, chunks[2], activities, border_style);
} // render_tasks

fn render_active_threads(frame: &mut Frame, area: Rect, live: &[ActivityEntry], border_style: Style) {
    let block = Block::default()
        .title(" Active Tools ")
        .borders(Borders::ALL)
        .border_style(border_style);
    
    let inner_area = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(live.iter().map(|_| Constraint::Length(2)).collect::<Vec<_>>())
        .split(inner_area);

    for (i, activity) in live.iter().enumerate() {
        let label = format!(" Thread {}: {} ", activity.thread_id.unwrap_or(0), activity.tool_name);
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
            .label(label)
            .percent(50); // Indeterminate or just "Started"
        
        frame.render_widget(gauge, chunks[i]);
    }
} // render_active_threads

fn render_task_list(frame: &mut Frame, area: Rect, tasks: &[Task], activities: &[ActivityEntry], border_style: Style) {
    let active_threads = activities.iter()
        .filter(|a| matches!(a.status, crate::tools::task::ActivityStatus::Starting))
        .count();
    
    let title = if active_threads > 0 {
        format!(" Tasks [Concurrency: {}] ", active_threads)
    } else {
        " Tasks ".to_string()
    };

    let block = Block::default()
        .title(title)
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
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(
                    format!("[{}] ", task.status),
                    Style::default().fg(color).add_modifier(Modifier::DIM),
                ),
                Span::styled(task.title.as_str(), Style::default().fg(Color::White)),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
} // render_task_list

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
        let start = activities.len().saturating_sub(20);
        for activity in &activities[start..] {
            let (icon, color) = match activity.status {
                crate::tools::task::ActivityStatus::Starting => ("▶", Color::Cyan),
                crate::tools::task::ActivityStatus::Success => ("✓", Color::Green),
                crate::tools::task::ActivityStatus::Failure => ("✗", Color::Red),
            };
            
            let time_str = activity.timestamp.format("%H:%M:%S").to_string();
            
            let mut spans = vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(format!("[{}] ", time_str), Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)),
            ];

            if let (Some(tid), Some(tot)) = (activity.thread_id, activity.total_threads) {
                spans.push(Span::styled(format!("{}/{} ", tid, tot), Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)));
            }

            spans.push(Span::styled(activity.tool_name.as_str(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(": ", Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(activity.summary.as_str(), Style::default().fg(Color::Gray)));

            lines.push(Line::from(spans));
        }
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: true });

    frame.render_widget(paragraph, area);
} // render_activity_log
