use std::collections::VecDeque;
use std::io::stdout;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use crossterm::{cursor, execute};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, List, ListItem, ListState, Paragraph, Wrap,
};
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::jockey::cli::Roles;
use crate::jockey::dag::TaskDag;
use crate::jockey::driver::{Governor, RunOutcome};
use crate::jockey::ledger::JockeyEvent;
use crate::jockey::worker::Worker;
use crate::sandbox::git::GitSandbox;

enum UiMsg {
    NeedsClarification(Vec<crate::jockey::planner::Clarification>),
    Planned(Box<TaskDag>),
    PlanError(String),
    RunEvent(Box<JockeyEvent>),
    RunFinished(#[allow(dead_code)] Box<RunOutcome>),
    RunError(String),
}

#[derive(Clone, PartialEq)]
enum Mode {
    Intake,
    Planning,
    Clarify,
    Confirm,
    Grinding,
    Done,
}

#[derive(Clone, Default)]
struct StepView {
    id: String,
    description: String,
    status: String,
    detail: String,
}

pub struct JockeyApp {
    mode: Mode,
    config: AppConfig,
    roles: Option<Roles>,
    repo: std::path::PathBuf,
    goal: String,
    input: String,
    dag: Option<TaskDag>,
    clarifications: Vec<crate::jockey::planner::Clarification>,
    answers: Vec<(String, String)>,
    clarify_index: usize,
    clarify_input: String,
    confirm_scroll: u16,
    steps: Vec<StepView>,
    telemetry: Vec<Line<'static>>,
    log: VecDeque<Line<'static>>,
    log_scroll: u16,
    step_list: ListState,
    run_id: Option<String>,
    branch: Option<String>,
    worktree: Option<std::path::PathBuf>,
    ledger: Option<crate::jockey::ledger::CostLedger>,
    status: String,
    quit_confirm: bool,
    planner_answers_rounds: u32,
}

const LOG_CAP: usize = 400;

pub async fn run_tui_new(config: AppConfig) -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let app = JockeyApp::new(config, None, repo);
    run_loop(app).await
}

pub async fn run_tui_with_dag(
    config: AppConfig,
    roles: Roles,
    goal: String,
    dag: TaskDag,
    repo: std::path::PathBuf,
) -> anyhow::Result<()> {
    let mut app = JockeyApp::new(config, Some(roles), repo);
    app.goal = goal;
    app.set_dag(dag);
    app.mode = Mode::Confirm;
    app.status = "review the plan — y to start, r to regenerate".into();
    run_loop(app).await
}

impl JockeyApp {
    fn new(
        config: AppConfig,
        roles: Option<Roles>,
        repo: std::path::PathBuf,
    ) -> Self {
        Self {
            mode: Mode::Intake,
            config,
            roles,
            repo,
            goal: String::new(),
            input: String::new(),
            dag: None,
            clarifications: Vec::new(),
            answers: Vec::new(),
            clarify_index: 0,
            clarify_input: String::new(),
            confirm_scroll: 0,
            steps: Vec::new(),
            telemetry: Vec::new(),
            log: VecDeque::new(),
            log_scroll: 0,
            step_list: ListState::default(),
            run_id: None,
            branch: None,
            worktree: None,
            ledger: None,
            status: "describe your goal and press Enter".into(),
            quit_confirm: false,
            planner_answers_rounds: 0,
        }
    }

    fn set_dag(&mut self, dag: TaskDag) {
        self.steps = dag
            .steps
            .iter()
            .map(|s| StepView {
                id: s.id.clone(),
                description: s.description.clone(),
                status: "pending".into(),
                detail: s.invariants.join(" | "),
            })
            .collect();
        self.dag = Some(dag);
    }

    fn push_log(&mut self, line: Line<'static>) {
        self.log.push_back(line);
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    fn start_planning(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        let Some(roles) = self.roles.clone() else {
            self.mode = Mode::Done;
            self.status =
                "no frontier provider configured; set [jj] frontier_provider"
                    .into();
            return;
        };
        self.mode = Mode::Planning;
        self.status = "frontier is decomposing the goal…".into();
        let goal = self.goal.clone();
        let repo = self.repo.clone();
        let answers = self.answers.clone();
        tokio::spawn(async move {
            let Some((frontier_client, frontier_model)) =
                roles.frontier.clone()
            else {
                let _ = tx.send(UiMsg::PlanError(
                    "no frontier provider configured".into(),
                ));
                return;
            };
            let planner = crate::jockey::planner::FrontierPlanner::new(
                frontier_client,
                frontier_model,
                0.2,
                8192,
            );
            let tree = crate::jockey::planner::collect_repo_tree(&repo, 400);
            match planner.plan(&goal, &answers, &tree).await {
                Ok(response) => match response.outcome {
                    crate::jockey::planner::PlanOutcome::Ready(dag) => {
                        let _ = tx.send(UiMsg::Planned(dag));
                    }
                    crate::jockey::planner::PlanOutcome::NeedsClarification(
                        qs,
                    ) => {
                        let _ = tx.send(UiMsg::NeedsClarification(qs));
                    }
                },
                Err(e) => {
                    let _ = tx.send(UiMsg::PlanError(e.to_string()));
                }
            }
        });
    }

    fn start_run(&mut self, tx: mpsc::UnboundedSender<UiMsg>) {
        let Some(dag) = self.dag.clone() else { return };
        let config = self.config.clone();
        let roles = self.roles.clone().expect("roles required to run");
        let goal = self.goal.clone();
        let repo = self.repo.clone();
        self.mode = Mode::Grinding;
        self.status = "grinding…".into();
        tokio::spawn(async move {
            let run_id = crate::jockey::cli::new_run_id();
            let _ =
                tx.send(UiMsg::RunEvent(Box::new(JockeyEvent::RunStarted {
                    run_id: run_id.clone(),
                    goal: goal.clone(),
                })));
            let sandbox = match GitSandbox::create(&repo, &run_id).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(UiMsg::RunError(e.to_string()));
                    return;
                }
            };
            let worker = Worker::new(
                roles.worker.0.clone(),
                roles.worker.1.clone(),
                config.jockey.worker_temperature,
                config.jockey.worker_max_tokens,
                std::env::var("SEEKR_WORKER_REASONING").ok(),
            );
            let governor = Governor::new(
                &config,
                worker,
                roles.frontier.clone(),
                roles.jev.clone(),
                sandbox,
                goal,
                dag,
                run_id,
            );
            let mut governor = match governor {
                Ok(g) => g,
                Err(e) => {
                    let _ = tx.send(UiMsg::RunError(e.to_string()));
                    return;
                }
            };
            let mut rx = governor.take_event_rx();
            let forward_tx = tx.clone();
            let forward = tokio::spawn(async move {
                while let Some(e) = rx.recv().await {
                    let _ = forward_tx.send(UiMsg::RunEvent(Box::new(e)));
                }
            });
            match governor.run_to_completion().await {
                Ok(outcome) => {
                    let _ = tx.send(UiMsg::RunFinished(Box::new(outcome)));
                }
                Err(e) => {
                    let _ = tx.send(UiMsg::RunError(e.to_string()));
                }
            }
            forward.abort();
        });
    }

    fn apply_run_event(&mut self, event: &JockeyEvent) {
        use crate::jockey::ledger::JockeyEvent::*;
        let dim = Style::default().fg(Color::DarkGray);
        let cyan = Style::default().fg(Color::Cyan);
        let green = Style::default().fg(Color::Green);
        let red = Style::default().fg(Color::Red);
        let yellow = Style::default().fg(Color::Yellow);

        match event {
            RunStarted { run_id, .. } => {
                self.run_id = Some(run_id.clone());
                self.push_log(Line::styled(
                    format!("run {run_id} started"),
                    cyan,
                ));
            }
            PlanReady { .. } => {}
            StepStarted { step_id, attempt } => {
                self.mark_step(
                    step_id,
                    "running",
                    format!("attempt {attempt}"),
                );
                self.telemetry = vec![Line::styled(
                    format!("step {step_id} — attempt {attempt}"),
                    Style::default().add_modifier(Modifier::BOLD),
                )];
                self.push_log(Line::styled(
                    format!("▶ {step_id} (attempt {attempt})"),
                    dim,
                ));
            }
            ActionProposed { tool, brief, .. } => {
                self.push_log(Line::from(vec![
                    Span::styled("  … ", dim),
                    Span::raw(format!("{tool}: {brief}")),
                ]));
            }
            ActionApproved {
                tool,
                scope_p,
                novelty,
            } => {
                let mut spans =
                    vec![Span::styled(format!("  ✓ {tool}"), green)];
                if let Some(s) = scope_p {
                    spans.push(Span::styled(format!(" scope {s:.2}"), cyan));
                }
                if let Some(n) = novelty {
                    spans.push(Span::styled(format!(" novelty {n:.1}"), cyan));
                }
                self.push_log(Line::from(spans));
            }
            ActionRejected { tool, reason } => {
                self.push_log(Line::from(vec![
                    Span::styled(format!("  ✗ {tool} "), red),
                    Span::styled(reason.clone(), yellow),
                ]));
            }
            ActionExecuted { ok, brief, .. } => {
                let (mark, style) =
                    if *ok { ("✔", green) } else { ("✘", red) };
                let mut line = vec![Span::styled(format!("  {mark} "), style)];
                if !*ok {
                    line.push(Span::styled(first_line(brief), yellow));
                }
                self.push_log(Line::from(line));
            }
            VerificationStarted { command, .. } => {
                self.push_log(Line::styled(
                    format!("  ⏳ verify: {command}"),
                    dim,
                ));
            }
            VerificationPassed { duration_secs, .. } => {
                self.push_log(Line::styled(
                    format!("  ✓ verified in {duration_secs:.1}s"),
                    green,
                ));
            }
            VerificationFailed { output, .. } => {
                self.push_log(Line::from(vec![
                    Span::styled("  ✗ verification failed: ", red),
                    Span::styled(first_line(output), yellow),
                ]));
            }
            RolledBack { to_commit, .. } => {
                self.push_log(Line::styled(
                    format!("  ↺ rolled back to {to_commit}"),
                    yellow,
                ));
            }
            Triage {
                verdict,
                confidence,
                ..
            } => {
                self.push_log(Line::styled(
                    format!("  ⚕ triage: {verdict} ({confidence:.2})"),
                    cyan,
                ));
            }
            Escalated { reason, .. } => {
                self.push_log(Line::styled(
                    format!("  ↑ escalate: {reason}"),
                    yellow,
                ));
            }
            FrontierGuidance { brief, .. } => {
                self.push_log(Line::styled(
                    format!("  ↑ frontier: {brief}"),
                    yellow,
                ));
            }
            StepCompleted { step_id, commit } => {
                let brief_commit = short(commit);
                self.mark_step(step_id, "verified", brief_commit.clone());
                self.push_log(Line::styled(
                    format!("✓ {step_id} checkpoint {brief_commit}"),
                    green,
                ));
            }
            StepFailed { step_id, reason } => {
                self.mark_step(step_id, "failed", reason.clone());
                self.push_log(Line::styled(
                    format!("✗ {step_id} failed: {reason}"),
                    red,
                ));
            }
            Usage { ledger } => {
                self.ledger = Some(ledger.clone());
            }
            RunCompleted {
                branch,
                worktree,
                ledger,
                ..
            } => {
                self.branch = Some(branch.clone());
                self.worktree = Some(worktree.clone());
                self.ledger = Some(ledger.clone());
                self.mode = Mode::Done;
                self.status = "run complete".into();
                self.push_log(Line::styled(
                    format!("✔ complete — branch {branch}"),
                    green,
                ));
            }
            RunFailed { reason, ledger } => {
                self.ledger = Some(ledger.clone());
                self.mode = Mode::Done;
                self.status = format!("run failed: {reason}");
                self.push_log(Line::styled(
                    format!("✗ run failed: {reason}"),
                    red,
                ));
            }
        }
    }

    fn mark_step(&mut self, id: &str, status: &str, detail: String) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.id == id) {
            step.status = status.to_string();
            step.detail = detail;
        }
    }

    fn handle_key(
        &mut self,
        key: KeyEvent,
        tx: &mpsc::UnboundedSender<UiMsg>,
    ) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('c')
        {
            return !self.quit_confirm || {
                self.quit_confirm = false;
                true
            };
        }
        match self.mode.clone() {
            Mode::Intake => match key.code {
                KeyCode::Enter => {
                    if self.input.trim().is_empty() {
                        self.status = "goal cannot be empty".into();
                    } else {
                        self.goal = self.input.trim().to_string();
                        self.input.clear();
                        self.start_planning(tx.clone());
                    }
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            },
            Mode::Clarify => {
                match key.code {
                    KeyCode::Enter => {
                        let idx = self.clarify_index;
                        if let Some(q) = self.clarifications.get(idx) {
                            self.answers.push((
                                q.id.clone(),
                                self.clarify_input.trim().to_string(),
                            ));
                            self.clarify_input.clear();
                            if idx + 1 < self.clarifications.len() {
                                self.clarify_index += 1;
                            } else {
                                self.planner_answers_rounds += 1;
                                if self.planner_answers_rounds >= 2 {
                                    self.status = "using best judgment with given answers".into();
                                }
                                self.start_planning(tx.clone());
                            }
                        }
                    }
                    KeyCode::Backspace => {
                        self.clarify_input.pop();
                    }
                    KeyCode::Char(c) => self.clarify_input.push(c),
                    _ => {}
                }
            }
            Mode::Confirm => match key.code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    self.start_run(tx.clone())
                }
                KeyCode::Char('r') => self.start_planning(tx.clone()),
                KeyCode::Char('e') => {
                    self.clarify_index = 0;
                    self.clarifications.clear();
                    self.answers.clear();
                    self.mode = Mode::Intake;
                }
                KeyCode::Char('q') | KeyCode::Esc => return true,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.confirm_scroll = self.confirm_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.confirm_scroll = self.confirm_scroll.saturating_sub(1)
                }
                _ => {}
            },
            Mode::Grinding => match key.code {
                KeyCode::Char('q') => {
                    if self.quit_confirm {
                        return true;
                    }
                    self.quit_confirm = true;
                    self.status = "press q again to detach (run continues in background of this process; Ctrl+C kills)".into();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.log_scroll = self.log_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.log_scroll = self.log_scroll.saturating_sub(1)
                }
                _ => {
                    self.quit_confirm = false;
                }
            },
            Mode::Done => match key.code {
                KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => {
                    return true;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.log_scroll = self.log_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.log_scroll = self.log_scroll.saturating_sub(1)
                }
                _ => {}
            },
            Mode::Planning => {}
        }
        false
    }

    fn handle_msg(&mut self, msg: UiMsg, tx: &mpsc::UnboundedSender<UiMsg>) {
        match msg {
            UiMsg::NeedsClarification(qs) => {
                self.clarifications = qs;
                self.clarify_index = 0;
                self.mode = Mode::Clarify;
                self.status = "answer the frontier's questions".into();
            }
            UiMsg::Planned(dag) => {
                self.set_dag(*dag);
                self.mode = Mode::Confirm;
                self.status =
                    "y start · r regenerate · e edit goal · q abort".into();
            }
            UiMsg::PlanError(e) => {
                self.mode = Mode::Intake;
                self.status = format!("planning failed: {e}");
            }
            UiMsg::RunEvent(e) => self.apply_run_event(&e),
            UiMsg::RunFinished(_) => {}
            UiMsg::RunError(e) => {
                self.mode = Mode::Done;
                self.status = format!("run error: {e}");
            }
        }
        let _ = tx;
    }
}

fn short(s: &str) -> String {
    s.chars().take(7).collect()
}

fn first_line(s: &str) -> String {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .to_string()
}

async fn run_loop(mut app: JockeyApp) -> anyhow::Result<()> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    let backend = ratatui::prelude::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (tx, mut rx) = mpsc::unbounded_channel::<UiMsg>();
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));

    let result: anyhow::Result<()> = loop {
        tokio::select! {
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if key.kind == crossterm::event::KeyEventKind::Press
                            && app.handle_key(key, &tx) {
                            break Ok(());
                        }
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => break Err(anyhow::anyhow!("input error: {e}")),
                    None => break Ok(()),
                }
            }
            Some(msg) = rx.recv() => {
                app.handle_msg(msg, &tx);
            }
            _ = tick.tick() => {}
        }
        if let Err(e) = terminal.draw(|f| render(f, &mut app)) {
            break Err(anyhow::anyhow!("render error: {e}"));
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    terminal.show_cursor()?;
    if let Some(worktree) = &app.worktree {
        println!("worktree: {}", worktree.display());
    }
    if let Some(branch) = &app.branch {
        println!("branch  : {branch}  (merge with: seekr merge <run-id>)");
    }
    if let Some(ledger) = &app.ledger {
        println!(
            "cost    : worker {} tok · jev {} calls · frontier {} calls",
            ledger.worker_prompt_tokens + ledger.worker_completion_tokens,
            ledger.jev_calls,
            ledger.frontier_calls
        );
    }
    println!("{}", app.status);
    result
}

fn render(f: &mut ratatui::Frame, app: &mut JockeyApp) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Line::from(vec![
        Span::styled(
            " ⚡ Jev Jockey ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" seekr {} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            match app.mode {
                Mode::Intake => "goal",
                Mode::Planning => "planning",
                Mode::Clarify => "clarify",
                Mode::Confirm => "plan review",
                Mode::Grinding => "grinding",
                Mode::Done => "finished",
            },
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            app.run_id
                .as_deref()
                .map(|r| format!("  {r}"))
                .unwrap_or_default(),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(title), rows[0]);

    match app.mode {
        Mode::Intake | Mode::Planning => render_intake(f, app, rows[1]),
        Mode::Clarify => render_clarify(f, app, rows[1]),
        Mode::Confirm => render_confirm(f, app, rows[1]),
        Mode::Grinding | Mode::Done => render_grind(f, app, rows[1]),
    }

    let status = Line::from(vec![Span::styled(
        format!(" {} ", app.status),
        Style::default().fg(Color::Black).bg(Color::Blue),
    )]);
    f.render_widget(Paragraph::new(status), rows[2]);
}

fn render_intake(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);
    let intro = Paragraph::new(vec![
        Line::from(Span::styled(
            "Describe the objective. The frontier model turns it into a rigid task DAG;",
            Style::default(),
        )),
        Line::from(Span::styled(
            "a local model then grinds through it under Jev supervision.",
            Style::default(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("repository: {}", app.repo.display()),
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(Block::default().borders(Borders::ALL).title(" goal "));
    f.render_widget(intro, chunks[0]);

    if app.mode == Mode::Planning {
        f.render_widget(
            Paragraph::new("planning…")
                .block(Block::default().borders(Borders::ALL).title(" status "))
                .style(Style::default().fg(Color::Yellow)),
            chunks[1],
        );
    } else {
        let input = Paragraph::new(format!("{}▏", app.input))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" your goal (Enter to plan) "),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(input, chunks[1]);
        f.set_cursor_position((
            chunks[1].x
                + 1
                + (app.input.chars().count() as u16
                    % (chunks[1].width.saturating_sub(2))),
            chunks[1].y + 1,
        ));
    }
    let roles_hint = match (
        &app.config.jockey.worker_provider,
        &app.config.jockey.frontier_provider,
    ) {
        (Some(w), Some(fr)) => format!("worker: {w} · frontier: {fr}"),
        (Some(w), None) => {
            format!("worker: {w} · frontier: NONE (planning needs --plan)")
        }
        _ => "roles unset — configure [jj] in config.toml".to_string(),
    };
    f.render_widget(
        Paragraph::new(roles_hint).style(Style::default().fg(Color::DarkGray)),
        chunks[2],
    );
}

fn render_clarify(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "The frontier needs a few answers before planning:",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    for (i, q) in app.clarifications.iter().enumerate() {
        let marker = if i < app.clarify_index {
            Span::styled(" ✓ ", Style::default().fg(Color::Green))
        } else if i == app.clarify_index {
            Span::styled(" ▶ ", Style::default().fg(Color::Cyan))
        } else {
            Span::styled("   ", Style::default())
        };
        let answered = app.answers.iter().find(|(id, _)| id == &q.id);
        lines.push(Line::from(vec![marker, Span::raw(q.question.clone())]));
        if let Some((_, a)) = answered {
            lines.push(Line::from(Span::styled(
                format!("     → {a}"),
                Style::default().fg(Color::DarkGray),
            )));
        } else if !q.why.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("     ({})", q.why),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(" clarify "))
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    let input = Paragraph::new(format!("{}▏", app.clarify_input)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" answer (Enter for next) "),
    );
    f.render_widget(input, chunks[1]);
    f.set_cursor_position((
        chunks[1].x + 1 + app.clarify_input.chars().count() as u16,
        chunks[1].y + 1,
    ));
}

fn render_confirm(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let Some(dag) = &app.dag else { return };
    let text = dag.render_summary();
    let lines: Vec<Line> = text.lines().map(Line::from).collect();
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(
            " task dag — y start · r regenerate · e edit goal · q abort ",
        ))
        .scroll((app.confirm_scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_grind(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let items: Vec<ListItem> = app
        .steps
        .iter()
        .map(|s| {
            let (icon, color) = match s.status.as_str() {
                "verified" | "completed" => ("✓", Color::Green),
                "running" => ("▶", Color::Cyan),
                "failed" | "rolled_back" => ("✗", Color::Red),
                _ => ("·", Color::DarkGray),
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{icon} "),
                        Style::default().fg(color),
                    ),
                    Span::styled(
                        s.id.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", s.detail),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Line::from(Span::styled(
                    format!("   {}", s.description),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();
    app.step_list.select(None);
    f.render_stateful_widget(
        List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" task dag ")),
        cols[0],
        &mut app.step_list,
    );

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(3)])
        .split(cols[1]);

    let mut telemetry: Vec<Line> = app.telemetry.clone();
    if telemetry.is_empty() {
        telemetry.push(Line::from(Span::styled(
            "waiting for first step",
            Style::default().fg(Color::DarkGray),
        )));
    }
    if let Some(l) = &app.ledger {
        telemetry.push(Line::from(String::new()));
        telemetry.push(Line::from(vec![
            Span::styled("tokens  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!(
                "worker {} · jev {} calls {} tok · frontier {} calls",
                l.worker_prompt_tokens + l.worker_completion_tokens,
                l.jev_calls,
                l.jev_input_tokens + l.jev_output_tokens,
                l.frontier_calls,
            )),
        ]));
    }
    if let (Some(b), Some(w)) = (&app.branch, &app.worktree) {
        telemetry.push(Line::from(vec![
            Span::styled("output  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{b} @ {}", w.display())),
        ]));
    }
    f.render_widget(
        Paragraph::new(telemetry)
            .block(Block::default().borders(Borders::ALL).title(" governor "))
            .wrap(Wrap { trim: false }),
        right[0],
    );

    let visible_height = right[1].height.saturating_sub(2) as usize;
    let total = app.log.len();
    let max_scroll = total.saturating_sub(visible_height);
    let scroll = app.log_scroll.min(max_scroll as u16);
    let shown: Vec<Line> =
        app.log.iter().skip(scroll as usize).cloned().collect();
    f.render_widget(
        Paragraph::new(shown)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" activity (j/k scroll) "),
            )
            .wrap(Wrap { trim: false }),
        right[1],
    );
}
