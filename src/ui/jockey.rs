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
use ratatui::layout::Margin;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Gauge, LineGauge, List, ListItem, Padding,
    Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    Wrap,
};
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::jockey::cli::Roles;
use crate::jockey::dag::TaskDag;
use crate::jockey::driver::{Governor, RunOutcome};
use crate::jockey::ledger::JockeyEvent;
use crate::jockey::worker::Worker;
use crate::sandbox::git::GitSandbox;
use crate::ui::menu::{self as control_menu, MenuOutcome, MenuState};

enum UiMsg {
    NeedsClarification(Vec<crate::jockey::planner::Clarification>),
    Planned(Box<TaskDag>),
    PlanError(String),
    RunEvent(Box<JockeyEvent>),
    RunFinished(#[allow(dead_code)] Box<RunOutcome>),
    RunError(String),
    MenuStatus(String),
}

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

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
    run_id: Option<String>,
    branch: Option<String>,
    worktree: Option<std::path::PathBuf>,
    ledger: Option<crate::jockey::ledger::CostLedger>,
    status: String,
    quit_confirm: bool,
    planner_answers_rounds: u32,
    menu: Option<MenuState>,
    help_visible: bool,
    attached_readonly: bool,
    tail_rx: Option<mpsc::UnboundedReceiver<JockeyEvent>>,
    tick: u64,
}

const LOG_CAP: usize = 400;

pub async fn run_tui_new(config: AppConfig) -> anyhow::Result<()> {
    let repo = std::env::current_dir()?;
    let roles = match crate::jockey::cli::resolve_roles(&config) {
        Ok(roles) => Some(roles),
        Err(e) => {
            eprintln!("configuration error: {e}");
            return Err(e);
        }
    };
    let mut app = JockeyApp::new(config, roles, repo);
    let frontier_ready = app
        .roles
        .as_ref()
        .and_then(|r| r.frontier.as_ref())
        .is_some();
    app.status = if frontier_ready {
        "describe your goal and press Enter".into()
    } else {
        "no frontier provider — set [jj] frontier_provider, or run with --plan"
            .into()
    };
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
            run_id: None,
            branch: None,
            worktree: None,
            ledger: None,
            status: "describe your goal and press Enter".into(),
            quit_confirm: false,
            planner_answers_rounds: 0,
            menu: None,
            help_visible: false,
            attached_readonly: false,
            tail_rx: None,
            tick: 0,
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
        let frontier_ready = self
            .roles
            .as_ref()
            .and_then(|r| r.frontier.as_ref())
            .is_some();
        if !frontier_ready {
            self.status =
                "no frontier provider configured — set [jj] frontier_provider, or start with --plan"
                    .into();
            return;
        }
        let roles = self.roles.clone().expect("frontier checked above");
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
        let Some(roles) = self.roles.clone() else {
            self.status =
                "provider roles unresolved; check config and restart".into();
            return;
        };
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
            if self.quit_confirm {
                return true;
            }
            self.quit_confirm = true;
            self.status = "press Ctrl+C again to quit".into();
            return false;
        }
        if self.quit_confirm {
            self.quit_confirm = false;
        }

        if self.menu.is_some() {
            let outcome = {
                let mut menu = self.menu.take().expect("checked");
                menu.provider_count = self.config.providers.len();
                let outcome = control_menu::handle_menu_key(
                    key,
                    &mut menu,
                    &mut self.config,
                );
                self.menu = Some(menu);
                outcome
            };
            return self.apply_menu_outcome(outcome, tx);
        }

        if self.help_visible {
            self.help_visible = false;
            return false;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('g')
        {
            let mut menu = MenuState::new();
            menu.provider_count = self.config.providers.len();
            self.menu = Some(menu);
            return false;
        }
        if key.code == KeyCode::Char('?') {
            self.help_visible = true;
            return false;
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
            UiMsg::MenuStatus(text) => {
                if let Some(menu) = self.menu.as_mut() {
                    menu.status = text;
                }
            }
        }
        let _ = tx;
    }

    fn apply_menu_outcome(
        &mut self,
        outcome: MenuOutcome,
        tx: &mpsc::UnboundedSender<UiMsg>,
    ) -> bool {
        match outcome {
            MenuOutcome::None => {}
            MenuOutcome::CloseMenu => {
                self.menu = None;
            }
            MenuOutcome::AttachRun(run_id) => {
                self.menu = None;
                self.attach_run(&run_id);
            }
            MenuOutcome::ResumeRun(run_id) => {
                self.menu = None;
                self.resume_run(&run_id, tx);
            }
            MenuOutcome::MergeRun(run_id) => {
                let tx_status = tx.clone();
                tokio::spawn(async move {
                    let result = crate::jockey::cli::merge_run(&run_id).await;
                    let msg = match result {
                        Ok(()) => format!("merged {run_id}"),
                        Err(e) => format!("merge failed: {e}"),
                    };
                    let _ = tx_status.send(UiMsg::MenuStatus(msg));
                });
            }
            MenuOutcome::CleanRun(run_id) => {
                let tx_status = tx.clone();
                tokio::spawn(async move {
                    let result = crate::jockey::cli::clean_run(&run_id).await;
                    let msg = match result {
                        Ok(()) => format!("cleaned {run_id} (branch kept)"),
                        Err(e) => format!("clean failed: {e}"),
                    };
                    let _ = tx_status.send(UiMsg::MenuStatus(msg));
                });
            }
            MenuOutcome::TestProvider(idx) => {
                let Some(p) = self.config.providers.get(idx).cloned() else {
                    return false;
                };
                let tx_status = tx.clone();
                tokio::spawn(async move {
                    let ok = crate::api::client::ApiClient::validate_key(
                        &p.key,
                        &p.base_url,
                        &p.model,
                    )
                    .await
                    .unwrap_or(false);
                    let msg = if ok {
                        format!("{} ✓ reachable", p.name)
                    } else {
                        format!("{} ✗ unreachable/invalid key", p.name)
                    };
                    let _ = tx_status.send(UiMsg::MenuStatus(msg));
                });
            }
            MenuOutcome::RestartRoles => {
                self.roles =
                    crate::jockey::cli::resolve_roles(&self.config).ok();
            }
        }
        false
    }

    /// View-only attach: replay the run's event log, then tail it live.
    fn attach_run(&mut self, run_id: &str) {
        let Ok(state) = crate::jockey::driver::RunState::load(run_id) else {
            self.status = format!("run {run_id} not found");
            return;
        };
        self.run_id = Some(state.run_id.clone());
        self.branch = Some(state.branch.clone());
        self.worktree = Some(state.worktree.clone());
        self.goal = state.goal.clone();
        self.set_dag(state.dag.clone());
        self.ledger = Some(state.ledger.clone());
        self.log.clear();
        self.telemetry.clear();

        let Some(path) = crate::jockey::driver::RunState::state_dir(run_id)
            .map(|d| d.join("events.jsonl"))
        else {
            self.status = format!("no state directory for {run_id}");
            return;
        };
        if let Ok(raw) = std::fs::read_to_string(&path) {
            for line in raw.lines() {
                if let Ok(event) = serde_json::from_str::<JockeyEvent>(line) {
                    self.apply_run_event(&event);
                }
            }
        }

        let (tail_tx, tail_rx) = mpsc::unbounded_channel::<JockeyEvent>();
        self.tail_rx = Some(tail_rx);
        tokio::spawn(async move {
            let mut position =
                std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if meta.len() < position {
                    position = 0;
                }
                if meta.len() > position {
                    use std::io::{Read, Seek, SeekFrom};
                    if let Ok(mut f) = std::fs::File::open(&path) {
                        let _ = f.seek(SeekFrom::Start(position));
                        let mut buf = String::new();
                        let _ = f.read_to_string(&mut buf);
                        for line in buf.lines() {
                            if let Ok(event) =
                                serde_json::from_str::<JockeyEvent>(line)
                            {
                                let _ = tail_tx.send(event);
                            }
                        }
                        position = meta.len();
                    }
                }
            }
        });

        self.attached_readonly = true;
        let finished = matches!(self.mode, Mode::Done);
        if !finished {
            self.mode = Mode::Grinding;
            self.status = format!(
                "attached to {run_id} (view-only) — Ctrl+G > runs > R resumes execution"
            );
        } else {
            self.status = format!("attached to {run_id} (finished)");
        }
    }

    /// Resume execution of an interrupted run.
    fn resume_run(&mut self, run_id: &str, tx: &mpsc::UnboundedSender<UiMsg>) {
        let tx = tx.clone();
        let state = match crate::jockey::driver::RunState::load(run_id) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("run {run_id} not found: {e}");
                return;
            }
        };
        let Some(roles) = self.roles.clone() else {
            self.status =
                "roles unresolved; fix providers in Ctrl+G first".into();
            return;
        };
        self.goal = state.goal.clone();
        self.run_id = Some(state.run_id.clone());
        self.attached_readonly = false;
        self.mode = Mode::Grinding;
        self.status = format!("resuming {run_id}…");
        let config = self.config.clone();
        tokio::spawn(async move {
            let _ =
                tx.send(UiMsg::RunEvent(Box::new(JockeyEvent::RunStarted {
                    run_id: state.run_id.clone(),
                    goal: state.goal.clone(),
                })));
            let sandbox = match crate::sandbox::git::GitSandbox::attach(
                &state.repo_root,
                &state.run_id,
            ) {
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
            let governor = Governor::attach(
                &config,
                worker,
                roles.frontier.clone(),
                roles.jev.clone(),
                sandbox,
                state,
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
            if let Err(e) = governor.run_to_completion().await {
                let _ = tx.send(UiMsg::RunError(e.to_string()));
            }
            forward.abort();
        });
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
            maybe_tail = async {
                match app.tail_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(event) = maybe_tail {
                    app.apply_run_event(&event);
                }
            }
            _ = tick.tick() => {
                app.tick = app.tick.wrapping_add(1);
            }
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

    f.render_widget(Paragraph::new(title_bar(app)), rows[0]);

    match app.mode {
        Mode::Intake | Mode::Planning => render_intake(f, app, rows[1]),
        Mode::Clarify => render_clarify(f, app, rows[1]),
        Mode::Confirm => render_confirm(f, app, rows[1]),
        Mode::Grinding => render_grind(f, app, rows[1]),
        Mode::Done => render_done(f, app, rows[1]),
    }

    if let Some(menu) = app.menu.as_mut() {
        crate::ui::menu::render_menu(f, menu, &app.config, rows[1]);
    }
    if app.help_visible {
        render_help(f, rows[1]);
    }

    f.render_widget(Paragraph::new(status_bar(app)), rows[2]);
}

fn title_bar(app: &JockeyApp) -> Line<'static> {
    let spinning = matches!(app.mode, Mode::Planning | Mode::Grinding);
    let spin = SPINNER[((app.tick / 3) % SPINNER.len() as u64) as usize];
    let mut spans = vec![
        Span::styled(
            " ⚡ Jev Jockey ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" v{} ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    if spinning {
        spans.push(Span::styled(
            format!("{spin} "),
            Style::default().fg(Color::Cyan),
        ));
    }
    spans.push(Span::styled(
        match app.mode {
            Mode::Intake => "goal",
            Mode::Planning => "planning",
            Mode::Clarify => "clarify",
            Mode::Confirm => "plan review",
            Mode::Grinding => "grinding",
            Mode::Done => "finished",
        },
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(run_id) = &app.run_id {
        spans.push(Span::styled(
            format!("  {run_id}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if app.attached_readonly {
        spans.push(Span::styled(
            "  👁 attached",
            Style::default().fg(Color::Magenta),
        ));
    }
    let repo = app
        .repo
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let used = spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum::<usize>()
        + repo.chars().count()
        + 1;
    let pad = area_width().saturating_sub(used);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled(
        format!("{repo} "),
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn area_width() -> usize {
    160
}

fn status_bar(app: &JockeyApp) -> Line<'static> {
    let (keys, _) = mode_hints(app);
    let mut spans = vec![Span::styled(
        format!(
            " {} ",
            if app.status.is_empty() {
                keys
            } else {
                app.status.as_str()
            }
        ),
        Style::default().fg(Color::Black).bg(if app.quit_confirm {
            Color::Red
        } else {
            Color::Blue
        }),
    )];
    spans.push(Span::styled(
        "  Ctrl+G menu · ? help · Ctrl+C quit",
        Style::default().fg(Color::DarkGray),
    ));
    Line::from(spans)
}

fn mode_hints(app: &JockeyApp) -> (&'static str, &'static str) {
    match app.mode {
        Mode::Intake => ("type a goal · Enter to plan", ""),
        Mode::Planning => ("frontier is decomposing the goal…", ""),
        Mode::Clarify => ("type an answer · Enter for next", ""),
        Mode::Confirm => (
            "y start · r regenerate · e edit goal · q abort · j/k scroll",
            "",
        ),
        Mode::Grinding => {
            ("j/k scroll · q detach · R resume (when attached)", "")
        }
        Mode::Done => ("q quit · Ctrl+G for past runs", ""),
    }
}

fn spinner_note(app: &JockeyApp, note: &str) -> Line<'static> {
    let spin = SPINNER[((app.tick / 3) % SPINNER.len() as u64) as usize];
    Line::from(vec![
        Span::styled(format!("{spin} "), Style::default().fg(Color::Cyan)),
        Span::raw(note.to_string()),
    ])
}

fn render_intake(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(area);

    let hero = vec![
        Line::from(Span::styled(
            "You bring the goal. The frontier plans it into a rigid DAG.",
            Style::default(),
        )),
        Line::from(Span::styled(
            "A local model grinds every step — Jev approves each action before it",
            Style::default(),
        )),
        Line::from(Span::styled(
            "touches disk — deterministic verification gates every step.",
            Style::default(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("repository  ", Style::default().fg(Color::Cyan)),
            Span::styled(
                app.repo.display().to_string(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("output      ", Style::default().fg(Color::Cyan)),
            Span::styled(
                "branch jj/<run-id> in an isolated worktree — main is never touched",
                Style::default().fg(Color::DarkGray),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(hero)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" welcome "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    if app.mode == Mode::Planning {
        f.render_widget(
            Paragraph::new(spinner_note(
                app,
                "frontier is decomposing the goal…",
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" status "),
            )
            .style(Style::default().fg(Color::Yellow)),
            chunks[1],
        );
    } else {
        let input = Paragraph::new(Line::from(vec![
            Span::raw(app.input.clone()),
            Span::styled("▏", Style::default().fg(Color::Cyan)),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(ratatui::symbols::border::ROUNDED)
                .title(" your goal (Enter to plan) "),
        )
        .wrap(Wrap { trim: false });
        f.render_widget(input, chunks[1]);
        let width = chunks[1].width.saturating_sub(2) as usize;
        let chars = app.input.chars().count();
        let (row, col) = (chars / width.max(1), chars % width.max(1));
        f.set_cursor_position((
            chunks[1].x + 1 + col as u16,
            chunks[1].y + 1 + row as u16,
        ));
    }

    let roles_hint = match (
        &app.config.jockey.worker_provider,
        &app.config.jockey.frontier_provider,
    ) {
        (Some(w), Some(fr)) => format!(
            "worker {w} · frontier {fr} · jev {}",
            if crate::jev::JevClient::from_env().unavailable().is_none() {
                "ready"
            } else {
                "UNAVAILABLE (writes blocked)"
            }
        ),
        (Some(w), None) => {
            format!("worker {w} · frontier NONE — set one in Ctrl+G to plan")
        }
        _ => "roles unset — open Ctrl+G ▸ providers".to_string(),
    };
    f.render_widget(
        Paragraph::new(Span::styled(
            roles_hint,
            Style::default().fg(Color::DarkGray),
        )),
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
            Span::raw("   ")
        };
        lines.push(Line::from(vec![marker, Span::raw(q.question.clone())]));
        if let Some((_, a)) = app.answers.iter().find(|(id, _)| id == &q.id) {
            lines.push(Line::from(Span::styled(
                format!("     ↳ {a}"),
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
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" clarify "),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );
    let input = Paragraph::new(Line::from(vec![
        Span::raw(app.clarify_input.clone()),
        Span::styled("▏", Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_set(ratatui::symbols::border::ROUNDED)
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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(3)])
        .split(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("goal  ", Style::default().fg(Color::Cyan)),
            Span::raw(app.goal.clone()),
        ]))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(ratatui::symbols::border::ROUNDED),
        )
        .wrap(Wrap { trim: true }),
        rows[0],
    );

    let text = dag.render_summary();
    let mut lines: Vec<Line> = Vec::new();
    for line in text.lines().skip(1) {
        if line.starts_with("  [") {
            lines.push(Line::from(vec![
                Span::styled(" ◆ ", Style::default().fg(Color::Cyan)),
                Span::raw(line.trim_start().to_string()),
            ]));
        } else if line.starts_with("    invariant:") {
            lines.push(Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    line.trim_start().to_string(),
                    Style::default().fg(Color::Yellow),
                ),
            ]));
        } else if !line.trim().is_empty() {
            lines.push(Line::from(line));
        }
    }
    let visible = rows[1].height.saturating_sub(2);
    let total = lines.len() as u16;
    let scroll = app.confirm_scroll.min(total.saturating_sub(visible));
    app.confirm_scroll = scroll;
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(
            " task dag — y start · r regenerate · e edit goal · q abort ",
        ))
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, rows[1]);
    render_scrollbar(f, rows[1], scroll, total);
}

fn render_grind(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    let mut items: Vec<ListItem> = Vec::new();
    let last = app.steps.len().saturating_sub(1);
    for (i, s) in app.steps.iter().enumerate() {
        let (icon, color) = match s.status.as_str() {
            "verified" | "completed" => ("✓", Color::Green),
            "running" => ("▶", Color::Cyan),
            "failed" | "rolled_back" => ("✗", Color::Red),
            _ => ("○", Color::DarkGray),
        };
        let connector = if i == 0 {
            "  ".to_string()
        } else if i == last {
            " └─".to_string()
        } else {
            " ├─".to_string()
        };
        items.push(ListItem::new(vec![
            Line::from(vec![
                Span::styled(connector, Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::styled(
                    s.id.clone(),
                    Style::default()
                        .fg(if s.status == "running" {
                            Color::Cyan
                        } else {
                            Color::Reset
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  {}", s.detail),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(Span::styled(
                format!("    {}", s.description),
                Style::default().fg(Color::DarkGray),
            )),
        ]));
    }
    if items.is_empty() {
        items.push(ListItem::new(Span::styled(
            "waiting for the plan…",
            Style::default().fg(Color::DarkGray),
        )));
    }
    let completed = app
        .steps
        .iter()
        .filter(|s| matches!(s.status.as_str(), "verified" | "completed"))
        .count();
    let dag_block = Block::default().borders(Borders::ALL).title(format!(
        " task dag {}/{} ",
        completed,
        app.steps.len()
    ));
    let list = List::new(items).block(dag_block);
    f.render_widget(list, cols[0]);

    let progress = if app.steps.is_empty() {
        0.0
    } else {
        completed as f64 / app.steps.len() as f64
    };
    let gauge = Gauge::default()
        .ratio(progress)
        .label(format!("{completed}/{} steps", app.steps.len()))
        .gauge_style(
            Style::default().fg(Color::Cyan).bg(Color::Rgb(16, 24, 32)),
        )
        .block(
            Block::default()
                .borders(Borders::NONE)
                .padding(Padding::horizontal(1)),
        );
    let gauge_area = Rect {
        x: cols[0].x,
        y: cols[0].y + cols[0].height.saturating_sub(1),
        width: cols[0].width,
        height: 1,
    };
    f.render_widget(gauge, gauge_area);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(11), Constraint::Min(3)])
        .split(cols[1]);

    let mut telemetry: Vec<Line> = vec![];
    if app.telemetry.is_empty() {
        telemetry.push(
            Span::styled(
                "waiting for first step",
                Style::default().fg(Color::DarkGray),
            )
            .into(),
        );
    } else {
        telemetry.extend(app.telemetry.clone());
    }
    if let Some(l) = &app.ledger {
        let worker_tok = l.worker_prompt_tokens + l.worker_completion_tokens;
        let jev_tok = l.jev_input_tokens + l.jev_output_tokens;
        let frontier_tok =
            l.frontier_prompt_tokens + l.frontier_completion_tokens;
        telemetry.push(Line::from("".to_string()));
        telemetry.push(Line::from(vec![
            Span::styled("cost  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("worker {}", fmt_tokens(worker_tok)),
                Style::default().fg(Color::Green),
            ),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("jev {} ({} calls)", fmt_tokens(jev_tok), l.jev_calls),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(" · ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "frontier {} ({} calls)",
                    fmt_tokens(frontier_tok),
                    l.frontier_calls
                ),
                Style::default().fg(Color::Magenta),
            ),
        ]));
    }
    f.render_widget(
        Paragraph::new(telemetry)
            .block(Block::default().borders(Borders::ALL).title(" governor "))
            .wrap(Wrap { trim: false }),
        right[0],
    );

    if let Some(l) = &app.ledger {
        let budget_rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(right[0]);
        let worker_tok = l.worker_prompt_tokens + l.worker_completion_tokens;
        let jev_tok = l.jev_input_tokens + l.jev_output_tokens;
        let frontier_tok =
            l.frontier_prompt_tokens + l.frontier_completion_tokens;
        let total_tok = (worker_tok + jev_tok + frontier_tok).max(1);
        let gauges = [
            (
                "local",
                worker_tok as f64 / total_tok as f64,
                Color::Green,
                fmt_tokens(worker_tok),
            ),
            (
                "jev",
                jev_tok as f64 / total_tok as f64,
                Color::Cyan,
                format!("{} · {} calls", fmt_tokens(jev_tok), l.jev_calls),
            ),
            (
                "frontier",
                l.frontier_calls as f64
                    / l.frontier_calls
                        .max(app.config.jockey.max_frontier_calls as u64)
                        .max(1) as f64,
                if l.frontier_calls as u32
                    >= app.config.jockey.max_frontier_calls
                {
                    Color::Red
                } else {
                    Color::Magenta
                },
                format!(
                    "{}/{} calls",
                    l.frontier_calls, app.config.jockey.max_frontier_calls
                ),
            ),
        ];
        for (i, (label, ratio, color, note)) in gauges.iter().enumerate() {
            let gauge = LineGauge::default()
                .label(Line::from(Span::styled(
                    format!(" {label:<8}{note:<16}"),
                    Style::default().fg(*color),
                )))
                .ratio(ratio.clamp(0.0, 1.0))
                .filled_style(
                    Style::default().fg(*color).add_modifier(Modifier::BOLD),
                )
                .unfilled_style(Style::default().fg(Color::DarkGray));
            f.render_widget(gauge, budget_rows[i]);
        }
    }

    let visible_height = right[1].height.saturating_sub(2) as usize;
    let total = app.log.len();
    let max_scroll = total.saturating_sub(visible_height);
    let scroll = (app.log_scroll as usize).min(max_scroll) as u16;
    app.log_scroll = scroll;
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
    render_scrollbar(f, right[1], scroll, total as u16);
}

fn render_done(f: &mut ratatui::Frame, app: &mut JockeyApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);

    let success = app.status.contains("complete");
    let banner = if success {
        "✔ RUN COMPLETE"
    } else {
        "✗ RUN STOPPED"
    };
    let color = if success { Color::Green } else { Color::Red };
    let steps_done = app
        .steps
        .iter()
        .filter(|s| matches!(s.status.as_str(), "verified" | "completed"))
        .count();
    let banner_lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            banner,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("{} of {} steps verified", steps_done, app.steps.len()),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(vec![
            Span::styled("goal  ", Style::default().fg(Color::Cyan)),
            Span::raw(app.goal.clone()),
        ]),
    ];
    f.render_widget(
        Paragraph::new(banner_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" run "),
            )
            .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(3)])
        .split(chunks[1]);

    if let Some(l) = &app.ledger {
        let worker_tok = l.worker_prompt_tokens + l.worker_completion_tokens;
        let jev_tok = l.jev_input_tokens + l.jev_output_tokens;
        let frontier_tok =
            l.frontier_prompt_tokens + l.frontier_completion_tokens;
        let worker_calls = l.worker_calls.to_string();
        let jev_calls =
            format!("{} (+{} cached)", l.jev_calls, l.jev_cache_hits);
        let frontier_calls = l.frontier_calls.to_string();
        let worker_tok_s = fmt_tokens(worker_tok);
        let jev_tok_s = fmt_tokens(jev_tok);
        let frontier_tok_s = fmt_tokens(frontier_tok);
        let table = Table::new(
            [
                Row::new(vec!["tier", "calls", "tokens"]).style(
                    Style::default()
                        .add_modifier(Modifier::BOLD)
                        .fg(Color::Cyan),
                ),
                Row::new(vec![
                    "local",
                    worker_calls.as_str(),
                    worker_tok_s.as_str(),
                ]),
                Row::new(vec!["jev", jev_calls.as_str(), jev_tok_s.as_str()]),
                Row::new(vec![
                    "frontier",
                    frontier_calls.as_str(),
                    frontier_tok_s.as_str(),
                ]),
            ],
            [
                Constraint::Length(12),
                Constraint::Length(18),
                Constraint::Length(18),
            ],
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" cost ledger "),
        );
        f.render_widget(table, rows[0]);
    }

    let visible_height = rows[1].height.saturating_sub(2) as usize;
    let total = app.log.len();
    let max_scroll = total.saturating_sub(visible_height);
    let scroll = (app.log_scroll as usize).min(max_scroll) as u16;
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
        rows[1],
    );

    if let (Some(b), Some(w)) = (&app.branch, &app.worktree) {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("merge  ", Style::default().fg(Color::Cyan)),
                Span::raw(format!(
                    "seekr merge {}   ·   worktree {}",
                    b.trim_start_matches("jj/"),
                    w.display()
                )),
            ])),
            chunks[2],
        );
    }
}

fn render_help(f: &mut ratatui::Frame, area: Rect) {
    let popup = centered_rect(area, 70, 70);
    f.render_widget(Clear, popup);
    let keys: [(&str, &str); 14] = [
        ("Ctrl+G", "control center (runs, providers, settings)"),
        ("?", "this help"),
        ("Ctrl+C ×2", "quit"),
        ("Enter", "submit goal / answer / confirm"),
        ("y / r / e", "start · regenerate · edit goal (plan review)"),
        ("q", "abort plan · detach view · quit (done)"),
        ("j / k, ↑↓", "scroll activity / navigate lists"),
        ("Tab, ←→", "switch control-center tabs"),
        ("n / e / d / t", "providers: add · edit · delete · test"),
        ("w / f", "assign worker / frontier role"),
        ("R", "resume an attached run's execution"),
        ("m / c", "runs: merge · clean worktree"),
        ("+ / -", "adjust [jj] settings"),
        ("Space", "toggle allow_degraded"),
    ];
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        "keys",
        Style::default()
            .add_modifier(Modifier::BOLD)
            .fg(Color::Cyan),
    ))];
    for (k, v) in keys {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{k:<14}", k = k),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(v),
        ]));
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_set(ratatui::symbols::border::ROUNDED)
                    .title(" help (Esc closes) "),
            )
            .wrap(Wrap { trim: false }),
        popup,
    );
}

fn render_scrollbar(
    f: &mut ratatui::Frame,
    area: Rect,
    position: u16,
    total: u16,
) {
    if total == 0 {
        return;
    }
    let mut scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));
    scrollbar = scrollbar.style(Style::default().fg(Color::DarkGray));
    f.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 0,
            horizontal: 0,
        }),
        &mut ScrollbarState::new(total.max(1) as usize)
            .position(position as usize),
    );
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
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

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
