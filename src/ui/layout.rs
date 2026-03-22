use ratatui::layout::{Constraint, Direction, Layout, Rect};

#[derive(Clone, Debug)]
pub struct AppLayout {
    pub title_bar: Rect,
    pub chat_panel: Rect,
    pub task_panel: Rect,
    pub input_bar: Rect,
    pub status_bar: Rect,
}

impl AppLayout {
    pub fn new(area: Rect) -> Self {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(5),
                Constraint::Length(5),
                Constraint::Length(1),
            ])
            .split(area);

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(vertical[1]);

        Self {
            title_bar: vertical[0],
            chat_panel: horizontal[0],
            task_panel: horizontal[1],
            input_bar: vertical[2],
            status_bar: vertical[3],
        }
    } // new
} // impl AppLayout

pub struct SetupLayout {
    pub header: Rect,
    pub content: Rect,
    pub footer: Rect,
}

impl SetupLayout {
    pub fn new(area: Rect) -> Self {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8),
                Constraint::Min(10),
                Constraint::Length(2),
            ])
            .split(area);

        Self {
            header: vertical[0],
            content: vertical[1],
            footer: vertical[2],
        }
    } // new
} // impl SetupLayout
