use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, Tabs, Wrap,
};

use crate::config::{AppConfig, ProviderConfig};
use crate::jockey::driver::RunState;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MenuTab {
    Runs,
    Providers,
    Settings,
    Help,
}

impl MenuTab {
    fn next(self) -> Self {
        match self {
            MenuTab::Runs => MenuTab::Providers,
            MenuTab::Providers => MenuTab::Settings,
            MenuTab::Settings => MenuTab::Help,
            MenuTab::Help => MenuTab::Runs,
        }
    }

    fn prev(self) -> Self {
        match self {
            MenuTab::Runs => MenuTab::Help,
            MenuTab::Providers => MenuTab::Runs,
            MenuTab::Settings => MenuTab::Providers,
            MenuTab::Help => MenuTab::Settings,
        }
    }
}

/// Inline editor for provider fields: name, base_url, model, key.
pub struct ProviderForm {
    pub editing: Option<usize>,
    pub fields: [String; 4],
    pub field: usize,
}

impl ProviderForm {
    pub fn from_provider(index: usize, p: &ProviderConfig) -> Self {
        Self {
            editing: Some(index),
            fields: [
                p.name.clone(),
                p.base_url.clone(),
                p.model.clone(),
                p.key.clone(),
            ],
            field: 0,
        }
    }

    pub fn blank() -> Self {
        Self {
            editing: None,
            fields: [
                String::new(),
                "https://api.openai.com/v1".into(),
                String::new(),
                String::new(),
            ],
            field: 0,
        }
    }

    fn label(&self) -> &'static str {
        match self.editing {
            Some(_) => "edit provider (empty key keeps existing)",
            None => "add provider",
        }
    }
}

pub struct MenuState {
    pub tab: MenuTab,
    pub runs_list: ListState,
    pub provider_list: ListState,
    pub settings_list: ListState,
    pub provider_form: Option<ProviderForm>,
    pub status: String,
    pub runs: Vec<(String, String)>,
    pub provider_count: usize,
}

impl MenuState {
    pub fn new() -> Self {
        let mut s = Self {
            tab: MenuTab::Runs,
            runs_list: ListState::default(),
            provider_list: ListState::default(),
            settings_list: ListState::default(),
            provider_form: None,
            status: String::new(),
            runs: RunState::list_runs(),
            provider_count: 0,
        };
        if !s.runs.is_empty() {
            s.runs_list.select(Some(s.runs.len() - 1));
        }
        s
    }

    pub fn refresh_runs(&mut self) {
        self.runs = RunState::list_runs();
        let selected = self.runs_list.selected().unwrap_or(0);
        self.runs_list
            .select(Some(selected.min(self.runs.len().saturating_sub(1))));
    }

    fn list_len(&self) -> usize {
        match self.tab {
            MenuTab::Runs => self.runs.len(),
            MenuTab::Providers => self.provider_count,
            MenuTab::Settings => SETTINGS_ROWS,
            MenuTab::Help => 0,
        }
    }

    fn selected_list(&mut self) -> &mut ListState {
        match self.tab {
            MenuTab::Runs => &mut self.runs_list,
            MenuTab::Providers => &mut self.provider_list,
            MenuTab::Settings => &mut self.settings_list,
            MenuTab::Help => &mut self.runs_list,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let list = self.selected_list();
        let current = list.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, len as i32 - 1) as usize;
        list.select(Some(next));
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// What the app should do after a menu keypress.
#[derive(Debug)]
pub enum MenuOutcome {
    None,
    CloseMenu,
    AttachRun(String),
    ResumeRun(String),
    MergeRun(String),
    CleanRun(String),
    TestProvider(usize),
    RestartRoles,
}

pub const SETTINGS_ROWS: usize = 8;

pub fn menu_hints(menu: &MenuState) -> String {
    if menu.provider_form.is_some() {
        return "Tab next field · Ctrl+S save · Esc cancel".to_string();
    }
    match menu.tab {
        MenuTab::Runs => {
            "Enter attach · R resume · m merge · c clean · u refresh · Tab tabs · Esc close"
                .to_string()
        }
        MenuTab::Providers => {
            "n add · e edit · d delete · t test key · w set worker · f set frontier · Esc close"
                .to_string()
        }
        MenuTab::Settings => "+/- adjust · Space toggle · s save · Esc close".to_string(),
        MenuTab::Help => "Esc close · Tab/←→ switch tabs".to_string(),
    }
}

pub fn handle_menu_key(
    key: KeyEvent,
    menu: &mut MenuState,
    config: &mut AppConfig,
) -> MenuOutcome {
    if let Some(form) = menu.provider_form.as_mut() {
        match key.code {
            KeyCode::Esc => {
                menu.provider_form = None;
                menu.status.clear();
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Enter => {
                form.field = (form.field + 1) % 4;
            }
            KeyCode::Backspace => {
                form.fields[form.field].pop();
            }
            KeyCode::Char('s')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let form = menu.provider_form.take().unwrap();
                let name = form.fields[0].trim().to_string();
                let base_url = form.fields[1].trim().to_string();
                let model = form.fields[2].trim().to_string();
                let supplied_key = form.fields[3].trim().to_string();
                if name.is_empty() || base_url.is_empty() || model.is_empty() {
                    menu.status =
                        "name, base_url and model are required".into();
                    menu.provider_form = Some(form);
                    return MenuOutcome::None;
                }
                match form.editing.and_then(|i| config.providers.get_mut(i)) {
                    Some(p) => {
                        let old_name = p.name.clone();
                        p.name = name.clone();
                        p.base_url = base_url;
                        p.model = model;
                        if !supplied_key.is_empty() {
                            p.key = supplied_key;
                        }
                        if config.jockey.worker_provider.as_deref()
                            == Some(old_name.as_str())
                        {
                            config.jockey.worker_provider = Some(name.clone());
                        }
                        if config.jockey.frontier_provider.as_deref()
                            == Some(old_name.as_str())
                        {
                            config.jockey.frontier_provider =
                                Some(name.clone());
                        }
                        menu.status = format!("updated {name}");
                    }
                    None => {
                        config.providers.push(ProviderConfig {
                            name: name.clone(),
                            key: supplied_key,
                            base_url,
                            model,
                            timeout: None,
                        });
                        menu.status = format!("added {name}");
                    }
                }
                let _ = config.save();
                menu.provider_count = config.providers.len();
                return MenuOutcome::RestartRoles;
            }
            KeyCode::Char(c) => form.fields[form.field].push(c),
            _ => {}
        }
        return MenuOutcome::None;
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('g')
    {
        return MenuOutcome::CloseMenu;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return MenuOutcome::CloseMenu,
        KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
            menu.tab = menu.tab.next()
        }
        KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
            menu.tab = menu.tab.prev()
        }
        KeyCode::Up | KeyCode::Char('k') => menu.move_selection(-1),
        KeyCode::Down | KeyCode::Char('j') => menu.move_selection(1),
        KeyCode::Char('1') => menu.tab = MenuTab::Runs,
        KeyCode::Char('2') => menu.tab = MenuTab::Providers,
        KeyCode::Char('3') => menu.tab = MenuTab::Settings,
        KeyCode::Char('4') => menu.tab = MenuTab::Help,
        _ => return menu.handle_tab_action(key, config),
    }
    MenuOutcome::None
}

impl MenuState {
    fn handle_tab_action(
        &mut self,
        key: KeyEvent,
        config: &mut AppConfig,
    ) -> MenuOutcome {
        match self.tab {
            MenuTab::Runs => {
                let Some(idx) = self.runs_list.selected() else {
                    return MenuOutcome::None;
                };
                let Some((id, _)) = self.runs.get(idx).cloned() else {
                    return MenuOutcome::None;
                };
                match key.code {
                    KeyCode::Enter | KeyCode::Char('a') => {
                        MenuOutcome::AttachRun(id)
                    }
                    KeyCode::Char('R') => MenuOutcome::ResumeRun(id),
                    KeyCode::Char('m') => MenuOutcome::MergeRun(id),
                    KeyCode::Char('c') => MenuOutcome::CleanRun(id),
                    KeyCode::Char('u') => {
                        self.refresh_runs();
                        self.status = format!("{} runs", self.runs.len());
                        MenuOutcome::None
                    }
                    _ => MenuOutcome::None,
                }
            }
            MenuTab::Providers => {
                let idx = self.provider_list.selected().unwrap_or(0);
                let Some(p) = config.providers.get(idx).cloned() else {
                    if matches!(key.code, KeyCode::Char('n')) {
                        self.provider_form = Some(ProviderForm::blank());
                    }
                    return MenuOutcome::None;
                };
                match key.code {
                    KeyCode::Char('n') => {
                        self.provider_form = Some(ProviderForm::blank());
                        MenuOutcome::None
                    }
                    KeyCode::Char('e') => {
                        self.provider_form =
                            Some(ProviderForm::from_provider(idx, &p));
                        MenuOutcome::None
                    }
                    KeyCode::Char('d') => {
                        let removed = &config.providers.remove(idx);
                        if config.jockey.worker_provider.as_deref()
                            == Some(removed.name.as_str())
                        {
                            config.jockey.worker_provider = None;
                        }
                        if config.jockey.frontier_provider.as_deref()
                            == Some(removed.name.as_str())
                        {
                            config.jockey.frontier_provider = None;
                        }
                        let _ = config.save();
                        self.provider_count = config.providers.len();
                        let sel = self.provider_list.selected().unwrap_or(0);
                        self.provider_list.select(Some(
                            sel.min(self.provider_count.saturating_sub(1)),
                        ));
                        self.status = format!("deleted {}", removed.name);
                        MenuOutcome::RestartRoles
                    }
                    KeyCode::Char('t') => MenuOutcome::TestProvider(idx),
                    KeyCode::Char('w') => {
                        config.jockey.worker_provider = Some(p.name.clone());
                        let _ = config.save();
                        self.status = format!("worker = {}", p.name);
                        MenuOutcome::RestartRoles
                    }
                    KeyCode::Char('f') => {
                        config.jockey.frontier_provider = Some(p.name.clone());
                        let _ = config.save();
                        self.status = format!("frontier = {}", p.name);
                        MenuOutcome::RestartRoles
                    }
                    _ => MenuOutcome::None,
                }
            }
            MenuTab::Settings => {
                let idx = self.settings_list.selected().unwrap_or(0);
                let jj = &mut config.jockey;
                let bumped =
                    |v: &mut u32, up: bool, step: u32, min: u32, max: u32| {
                        if up {
                            *v = (*v).saturating_add(step).min(max);
                        } else {
                            *v = (*v).saturating_sub(step).max(min);
                        }
                    };
                if key.code == KeyCode::Char('s') {
                    let _ = config.save();
                    self.status = "settings saved".into();
                    return MenuOutcome::None;
                }
                match (idx, key.code) {
                    (0, KeyCode::Char('+')) => {
                        bumped(&mut jj.max_attempts_per_step, true, 1, 1, 20)
                    }
                    (0, KeyCode::Char('-')) => {
                        bumped(&mut jj.max_attempts_per_step, false, 1, 1, 20)
                    }
                    (1, KeyCode::Char('+')) => {
                        bumped(&mut jj.max_frontier_calls, true, 1, 0, 50)
                    }
                    (1, KeyCode::Char('-')) => {
                        bumped(&mut jj.max_frontier_calls, false, 1, 0, 50)
                    }
                    (2, KeyCode::Char('+')) => {
                        jj.verification_timeout_secs =
                            jj.verification_timeout_secs.saturating_add(60);
                    }
                    (2, KeyCode::Char('-')) => {
                        jj.verification_timeout_secs = jj
                            .verification_timeout_secs
                            .saturating_sub(60)
                            .max(30);
                    }
                    (3, KeyCode::Char('+')) => {
                        jj.step_timeout_secs =
                            jj.step_timeout_secs.saturating_add(300);
                    }
                    (3, KeyCode::Char('-')) => {
                        jj.step_timeout_secs =
                            jj.step_timeout_secs.saturating_sub(300).max(300);
                    }
                    (4, KeyCode::Char('+')) => {
                        jj.worker_temperature =
                            (jj.worker_temperature + 0.1).min(1.5);
                    }
                    (4, KeyCode::Char('-')) => {
                        jj.worker_temperature =
                            (jj.worker_temperature - 0.1).max(0.0);
                    }
                    (5, KeyCode::Char('+')) => {
                        jj.worker_max_tokens = jj
                            .worker_max_tokens
                            .saturating_add(1024)
                            .min(32768);
                    }
                    (5, KeyCode::Char('-')) => {
                        jj.worker_max_tokens =
                            jj.worker_max_tokens.saturating_sub(1024).max(1024);
                    }
                    (6, KeyCode::Char('+')) => {
                        jj.scope_threshold =
                            (jj.scope_threshold + 0.05).min(0.99);
                    }
                    (6, KeyCode::Char('-')) => {
                        jj.scope_threshold =
                            (jj.scope_threshold - 0.05).max(0.5);
                    }
                    (7, KeyCode::Char(' ')) => {
                        jj.allow_degraded = !jj.allow_degraded;
                    }
                    _ => {}
                }
                let _ = config.save();
                MenuOutcome::None
            }
            MenuTab::Help => MenuOutcome::None,
        }
    }
}

pub fn render_menu(
    f: &mut ratatui::Frame,
    menu: &mut MenuState,
    config: &AppConfig,
    area: Rect,
) {
    let popup = centered(area, 92, 88);
    f.render_widget(Clear, popup);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(popup);

    let tabs = Tabs::new(vec![" runs ", " providers ", " settings ", " help "])
        .select(match menu.tab {
            MenuTab::Runs => 0,
            MenuTab::Providers => 1,
            MenuTab::Settings => 2,
            MenuTab::Help => 3,
        })
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" control center (Ctrl+G) "),
        );
    f.render_widget(tabs, rows[0]);

    match menu.tab {
        MenuTab::Runs => {
            let items: Vec<ListItem> = menu
                .runs
                .iter()
                .map(|(id, status)| {
                    let color = if status.starts_with("success") {
                        Color::Green
                    } else if status.starts_with("failed")
                        || status.starts_with("unknown")
                    {
                        Color::Red
                    } else {
                        Color::Yellow
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            id.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(
                            status.clone(),
                            Style::default().fg(color),
                        ),
                    ]))
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(format!(
                    " runs ({}) — Enter attach · R resume · m merge · c clean ",
                    menu.runs.len()
                )))
                .highlight_style(Style::default().bg(Color::DarkGray));
            f.render_stateful_widget(list, rows[1], &mut menu.runs_list);
        }
        MenuTab::Providers => {
            let items: Vec<ListItem> = config
                .providers
                .iter()
                .map(|p| {
                    let worker = config.jockey.worker_provider.as_deref()
                        == Some(p.name.as_str());
                    let frontier = config.jockey.frontier_provider.as_deref()
                        == Some(p.name.as_str());
                    let mut spans = vec![
                        Span::styled(
                            format!("{:<16}", p.name),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("{:<34}", p.model),
                            Style::default().fg(Color::Cyan),
                        ),
                    ];
                    if worker {
                        spans.push(Span::styled(
                            "[worker] ",
                            Style::default().fg(Color::Green),
                        ));
                    }
                    if frontier {
                        spans.push(Span::styled(
                            "[frontier]",
                            Style::default().fg(Color::Yellow),
                        ));
                    }
                    if p.key.is_empty()
                        && !p.base_url.contains("localhost")
                        && !p.base_url.contains("server")
                    {
                        spans.push(Span::styled(
                            "  no key",
                            Style::default().fg(Color::Red),
                        ));
                    }
                    spans.push(Span::styled(
                        format!("  {}", p.base_url),
                        Style::default().fg(Color::DarkGray),
                    ));
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let list = List::new(items).block(
                Block::default().borders(Borders::ALL).title(
                    " providers — n add · e edit · d delete · t test · w worker · f frontier ",
                ),
            );
            f.render_stateful_widget(list, rows[1], &mut menu.provider_list);
        }
        MenuTab::Settings => {
            let jj = &config.jockey;
            let rows_data = [
                (
                    "max_attempts_per_step",
                    jj.max_attempts_per_step.to_string(),
                    "1-20",
                ),
                (
                    "max_frontier_calls",
                    jj.max_frontier_calls.to_string(),
                    "0-50",
                ),
                (
                    "verification_timeout_secs",
                    jj.verification_timeout_secs.to_string(),
                    "±60",
                ),
                (
                    "step_timeout_secs",
                    jj.step_timeout_secs.to_string(),
                    "±300",
                ),
                (
                    "worker_temperature",
                    format!("{:.2}", jj.worker_temperature),
                    "±0.1",
                ),
                (
                    "worker_max_tokens",
                    jj.worker_max_tokens.to_string(),
                    "±1024",
                ),
                (
                    "scope_threshold",
                    format!("{:.2}", jj.scope_threshold),
                    "±0.05",
                ),
                (
                    "allow_degraded",
                    if jj.allow_degraded { "true" } else { "false" }
                        .to_string(),
                    "Space",
                ),
            ];
            let items: Vec<ListItem> = rows_data
                .iter()
                .map(|(k, v, hint)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{k:<26}"),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::styled(
                            format!("{v:<10}"),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("({hint})"),
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                })
                .collect();
            let list =
                List::new(items)
                    .block(Block::default().borders(Borders::ALL).title(
                    " [jj] settings — +/- adjust · Space toggle (auto-saves) ",
                ));
            f.render_stateful_widget(list, rows[1], &mut menu.settings_list);
        }
        MenuTab::Help => {
            let help = vec![
                ("global", ""),
                ("Ctrl+G", "open / close the control center"),
                ("?", "toggle this help"),
                ("Ctrl+C", "quit (press twice)"),
                ("", ""),
                ("intake", "type goal · Enter to plan"),
                ("clarify", "type answer · Enter for next"),
                (
                    "confirm",
                    "y start · r regenerate · e edit goal · q abort · j/k scroll",
                ),
                ("grinding", "j/k scroll activity · q detach view"),
                ("done", "q quit"),
                ("", ""),
                (
                    "runs tab",
                    "Enter attach (view-only) · R resume · m merge · c clean · u refresh",
                ),
                (
                    "providers",
                    "n add · e edit · d delete · t test key · w/f assign roles",
                ),
                ("settings", "+/- adjust · Space toggle allow_degraded"),
            ];
            let lines: Vec<Line> = help
                .into_iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(
                            format!("{k:<12}"),
                            Style::default().fg(Color::Cyan),
                        ),
                        Span::raw(v),
                    ])
                })
                .collect();
            f.render_widget(
                Paragraph::new(lines)
                    .block(
                        Block::default().borders(Borders::ALL).title(" keys "),
                    )
                    .wrap(Wrap { trim: false }),
                rows[1],
            );
        }
    }

    let status = if menu.status.is_empty() {
        menu_hints(menu)
    } else {
        menu.status.clone()
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {status} "),
            Style::default().fg(Color::Black).bg(Color::Blue),
        ))),
        rows[2],
    );

    if let Some(form) = &menu.provider_form {
        let form_area = centered(popup, 70, 50);
        f.render_widget(Clear, form_area);
        let labels = ["name", "base_url", "model", "api key"];
        let mut lines: Vec<Line> = vec![Line::from(Span::styled(
            form.label(),
            Style::default().add_modifier(Modifier::BOLD),
        ))];
        for (i, label) in labels.iter().enumerate() {
            let marker = if i == form.field { "▶ " } else { "  " };
            lines.push(Line::from(vec![
                Span::raw(marker.to_string()),
                Span::styled(
                    format!("{label:<10}"),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(if i == 3 {
                    mask_key(&form.fields[3])
                } else {
                    form.fields[i].clone()
                }),
                Span::styled(
                    if i == form.field { "▏" } else { "" },
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }
        lines.push(Line::from(Span::styled(
            "Tab next · Ctrl+S save · Esc cancel",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default().borders(Borders::ALL).title(" provider "),
                )
                .wrap(Wrap { trim: false }),
            form_area,
        );
    }
}

fn mask_key(key: &str) -> String {
    if key.len() <= 6 {
        "•".repeat(key.len())
    } else {
        format!("{}{}", &key[..4], "•".repeat(key.len().saturating_sub(4)))
    }
}

fn centered(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let h = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(v[1]);
    h[1]
}
