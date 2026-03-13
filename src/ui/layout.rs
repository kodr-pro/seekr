// ui/layout.rs - Layout management for the TUI
//
// Defines the main application layout with title bar, chat panel,
// task panel, input bar, and status bar.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// The computed layout areas for the main application
pub struct AppLayout {
    pub title_bar: Rect,
    pub chat_panel: Rect,
    pub task_panel: Rect,
    pub input_bar: Rect,
    pub status_bar: Rect,
}

impl AppLayout {
    /// Compute layout areas from the terminal size
    pub fn new(area: Rect) -> Self {
        // Vertical: title_bar | main_area | input_bar | status_bar
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // title bar
                Constraint::Min(5),    // main area (chat + tasks)
                Constraint::Length(3), // input bar
                Constraint::Length(1), // status bar
            ])
            .split(area);

        // Horizontal split of main area: chat (~70%) | tasks (~30%)
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(70),
                Constraint::Percentage(30),
            ])
            .split(vertical[1]);

        Self {
            title_bar: vertical[0],
            chat_panel: horizontal[0],
            task_panel: horizontal[1],
            input_bar: vertical[2],
            status_bar: vertical[3],
        }
    }
}

/// Layout for the setup wizard
#[allow(dead_code)]
pub struct SetupLayout {
    pub header: Rect,
    pub content: Rect,
    pub footer: Rect,
}

impl SetupLayout {
    #[allow(dead_code)]
    pub fn new(area: Rect) -> Self {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8), // header with SEEKR art
                Constraint::Min(10),   // content area
                Constraint::Length(2), // footer
            ])
            .split(area);

        Self {
            header: vertical[0],
            content: vertical[1],
            footer: vertical[2],
        }
    }
}
