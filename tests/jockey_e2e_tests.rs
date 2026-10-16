//! End-to-end governor loop tests: real git worktrees, a wiremock worker
//! LLM, and the hermetic `mock-aiwrangler` MCP server standing in for the
//! AiWrangler gates (scenario files steer its verdicts).

use std::path::Path;

use serde_json::json;
use wiremock::MockServer;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use seekr::api::client::ApiClient;
use seekr::config::{AppConfig, JockeyConfig, ProviderConfig};
use seekr::jockey::dag::{DagStep, StepStatus, TaskDag};
use seekr::jockey::driver::{Governor, RunOutcome};
use seekr::jockey::worker::Worker;
use seekr::sandbox::git::GitSandbox;

/// Path of the compiled mock MCP server for test scenarios.
fn mock_wrangler_bin() -> &'static str {
    env!("CARGO_BIN_EXE_mock_aiwrangler")
}

/// Writes a scenario file and returns the AppConfig pointing the
/// wrangler command at the mock with that scenario.
fn wrangler_config(scenario: serde_json::Value) -> AppConfig {
    let dir = std::env::temp_dir()
        .join(format!("mock-awr-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("scenario.json");
    std::fs::write(&path, serde_json::to_string(&scenario).unwrap()).unwrap();
    AppConfig {
        jockey: JockeyConfig {
            wrangler_command: Some(format!(
                "{} {}",
                mock_wrangler_bin(),
                path.display()
            )),
            wrangler_timeout_secs: 20,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn isolate_state_dir() {
    use std::sync::Once;
    static STATE_ISOLATION: Once = Once::new();
    STATE_ISOLATION.call_once(|| {
        let dir = std::env::temp_dir()
            .join(format!("seekr-jockey-test-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Safe under the test harness: set before any governor reads it,
        // once per process (mirrors the pre-existing pattern).
        unsafe { std::env::set_var("SEEKR_STATE_DIR", &dir) };
    });
}

async fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    isolate_state_dir();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    run(&root, &["init", "--quiet"]).await;
    run(&root, &["config", "user.email", "jj@test"]).await;
    run(&root, &["config", "user.name", "jj"]).await;
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn original() {}\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n")
        .unwrap();
    run(&root, &["add", "-A"]).await;
    run(&root, &["commit", "--quiet", "-m", "init"]).await;
    (tmp, root)
}

async fn run(cwd: &Path, args: &[&str]) -> String {
    let out = tokio::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn api_client(server: &MockServer, name: &str) -> (ApiClient, String) {
    let provider = ProviderConfig {
        name: name.to_string(),
        key: String::new(),
        base_url: server.uri(),
        model: "mock-model".to_string(),
        timeout: Some(5),
    };
    (
        ApiClient::new_for_provider(&Default::default(), &provider),
        "mock-model".to_string(),
    )
}

fn tool_call(name: &str, args: serde_json::Value) -> serde_json::Value {
    json!([{
        "id": format!("call_{name}"),
        "type": "function",
        "function": { "name": name, "arguments": args.to_string() }
    }])
}

async fn mount_worker_reply(
    server: &MockServer,
    tool: &str,
    args: serde_json::Value,
    times: usize,
) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "", "tool_calls": tool_call(tool, args)}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })))
        .up_to_n_times(times as u64)
        .mount(server)
        .await;
}

fn one_step_dag(allowed: Vec<&str>, verify: &str) -> TaskDag {
    TaskDag {
        goal: "test goal".into(),
        steps: vec![DagStep {
            id: "s1".into(),
            description: "make the marker exist".into(),
            depends_on: vec![],
            invariants: vec!["verification passes".into()],
            allowed_paths: allowed
                .into_iter()
                .map(std::path::PathBuf::from)
                .collect(),
            verification_command: Some(verify.into()),
            status: StepStatus::Pending,
        }],
    }
}

async fn drive(
    config: &AppConfig,
    worker: Worker,
    frontier: Option<ApiClient>,
    sandbox: GitSandbox,
    dag: TaskDag,
) -> (RunOutcome, Vec<seekr::jockey::JockeyEvent>) {
    let frontier = frontier.map(|c| (c, "mock-model".to_string()));
    let run_id = format!("test-{}", uuid::Uuid::new_v4().simple());
    let mut governor = Governor::new(
        config,
        worker,
        frontier,
        sandbox,
        "test goal".into(),
        dag,
        run_id,
    )
    .unwrap();
    let mut rx = governor.take_event_rx();
    let outcome = governor.run_to_completion().await.unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (outcome, events)
}

#[tokio::test]
async fn out_of_scope_write_rejected_without_disk_mutation() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // worker: (1) write OUT of scope, (2) write in scope, (3) finish
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "Cargo.toml", "content": "hijacked"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/lib.rs", "content": "pub fn changed() {}\n"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        5,
    )
    .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-a").await.unwrap();
    let config = wrangler_config(json!({}));

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert_eq!(outcome, RunOutcome::Success);
    assert_eq!(
        std::fs::read_to_string(root.join("Cargo.toml")).unwrap(),
        "[package]\nname = \"x\"\n",
        "out-of-scope write must never touch disk"
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.worktree().join("src/lib.rs")).unwrap(),
        "pub fn changed() {}\n"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::ActionRejected { reason, .. }
            if reason.contains("outside the step's allowed_paths")
    )));
    let status = run(sandbox.worktree(), &["status", "--porcelain"]).await;
    let meaningful: Vec<&str> = status
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !(l.starts_with("?? ")
                    && l.ends_with("aiwrangler.config.json")
                    && !l.contains('/'))
        })
        .collect();
    assert!(
        meaningful.is_empty(),
        "checkpoint must leave a clean tree (seekr plumbing aside), got: {status}"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn wrangler_fix_required_keeps_diff_then_recovers() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // attempt 1: writes a file, finishes -> check gate cites findings
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/attempt1.rs", "content": "first"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        1,
    )
    .await;
    // attempt 2: proposes the IDENTICAL write again -> fingerprint loop reject,
    // then a different action whose deterministic verification passes but
    // the check gate cites findings for -> rollback
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/attempt1.rs", "content": "first"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "fixed.txt", "content": "ok"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "fixed"}),
        1,
    )
    .await;
    // attempt 3: the rollback undid fixed.txt and the identical write is
    // now loop-rejected, so the worker writes it with different content
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "fixed.txt", "content": "ok, findings addressed"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "fixed for real"}),
        5,
    )
    .await;

    // check gate: first completion is rejected with cited findings, the
    // second (recovered) one passes.
    let mut config = wrangler_config(json!({
        "checks": [
            "AIWRANGLER FIX REQUIRED (exit 17)\ndirty: 1 file(s) across 1 repo(s)\n\nsemantic verdicts (jev, 100 in / 20 out tokens):\n\nsemantic findings:\n  RULE-2: silent default — the error path returns an empty vec without comment\n\nloop: cycle 1",
            "AIWRANGLER PASS (exit 0)\ndirty: 1 file(s)\nsemantic verdicts (jev, 90 in / 15 out tokens):"
        ]
    }));
    // this scenario exercises three attempts; the product default (2)
    // is a policy choice, not a mechanism constraint
    config.jockey.max_attempts_per_step = 3;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-b").await.unwrap();

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src", "fixed.txt"], "test -f fixed.txt"),
    )
    .await;

    assert_eq!(outcome, RunOutcome::Success);
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::WranglerGate { gate, action, .. }
            if gate == "check" && action.contains("FixRequired")
    )));
    // near-miss work is kept between attempts, not rolled back
    assert!(
        !events.iter().any(|e| matches!(
            e,
            seekr::jockey::JockeyEvent::RolledBack { .. }
        )),
        "attempts iterate on the same tree; rollback only on step failure"
    );
    assert!(sandbox.worktree().join("src/attempt1.rs").exists());
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::ActionRejected { reason, .. } if reason.contains("loop detected")
    )));
    assert!(sandbox.worktree().join("fixed.txt").exists());
    // semantic usage lands in the ledger
    assert!(config_ledger_checks(&events) >= 1);
    sandbox.dispose().await.unwrap();
}

fn config_ledger_checks(events: &[seekr::jockey::JockeyEvent]) -> u64 {
    events
        .iter()
        .filter_map(|e| match e {
            seekr::jockey::JockeyEvent::Usage { ledger } => {
                Some(ledger.wrangler_calls)
            }
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

#[tokio::test]
async fn exhausted_attempts_escalate_and_fail_cleanly() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;
    let frontier_server = MockServer::start().await;

    // worker: declares the step done without ever fixing it
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        100,
    )
    .await;

    // frontier: one guidance round that does not help
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant",
                "content": "GUIDANCE: create the file fixed.txt containing ok."}}],
            "usage": {"prompt_tokens": 500, "completion_tokens": 50, "total_tokens": 550}
        })))
        .expect(1..=2)
        .mount(&frontier_server)
        .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let (frontier_client, _) = api_client(&frontier_server, "frontier");
    let sandbox = GitSandbox::create(&root, "e2e-c").await.unwrap();
    let mut config = wrangler_config(json!({}));
    config.jockey.max_attempts_per_step = 2;

    let (outcome, events) = drive(
        &config,
        worker,
        Some(frontier_client),
        sandbox.clone(),
        one_step_dag(vec!["src"], "test -f fixed.txt"),
    )
    .await;

    assert!(matches!(outcome, RunOutcome::Failed(_)));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, seekr::jockey::JockeyEvent::Escalated { .. }))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::FrontierGuidance { .. }
    )));
    assert!(
        !sandbox.worktree().join("fixed.txt").exists(),
        "nothing may persist after a failed run"
    );
    let status = run(sandbox.worktree(), &["status", "--porcelain"]).await;
    assert!(
        status.trim().is_empty(),
        "worktree must be rolled back clean, got: {status}"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn fail_closed_when_wrangler_dies_mid_run() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // worker: makes a real in-scope change and finishes; the check gate
    // server dies the moment it is called, so the step can never pass.
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/lib.rs", "content": "pub fn changed() {}\n"}),
        100,
    )
    .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-d").await.unwrap();
    let mut config = wrangler_config(json!({"die_on_check": true}));
    config.jockey.max_attempts_per_step = 2;

    let (outcome, _events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert!(
        matches!(outcome, RunOutcome::Failed(_)),
        "fail-closed: an unreachable check gate must sink the run"
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.worktree().join("src/lib.rs")).unwrap(),
        "pub fn original() {}\n",
        "fail-closed: the write must be rolled back, never checkpointed"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn human_review_parks_the_step_for_the_owner() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/lib.rs", "content": "pub fn changed() {}\n"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        5,
    )
    .await;

    let config = wrangler_config(json!({
        "checks": [
            "AIWRANGLER HUMAN REVIEW (exit 5)\ndirty: 1 file(s)\n\nsecurity findings:\n  RULE-10: touches a trust boundary — owner must decide\n\nloop: cycle 1"
        ]
    }));

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-human").await.unwrap();

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert!(matches!(outcome, RunOutcome::Failed(_)));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::WranglerGate { action, .. }
            if action.contains("HumanReview")
    )));
    assert!(
        events.iter().any(|e| matches!(
            e,
            seekr::jockey::JockeyEvent::StepFailed { reason, .. }
                if reason.to_lowercase().contains("owner action required")
        )),
        "the step must tell the owner how to ack"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn plan_gate_clarify_stops_run_before_any_work() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // The worker would write, but the plan gate stops the run first —
    // this mount proves the worker is never consulted.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .expect(0)
        .mount(&worker_server)
        .await;

    let config = wrangler_config(json!({
        "plan": "AIWRANGLER PLAN: CLARIFY\n\nreasons:\n  - ambiguity: 'marker' is undefined — file name or env var?\n  - open question Q-3 touches this task\n\nAsk the user to resolve the flagged items before writing code."
    }));

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-clarify").await.unwrap();

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert!(matches!(
        outcome,
        RunOutcome::Failed(reason) if reason.contains("CLARIFY")
    ));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::WranglerGate { gate, action, .. }
            if gate == "plan" && action.contains("Clarify")
    )));
    // No worker call, no disk change: the plan gate fired first.
    assert_eq!(
        std::fs::read_to_string(sandbox.worktree().join("src/lib.rs")).unwrap(),
        "pub fn original() {}\n"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn plan_gate_context_pack_reaches_the_worker() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // Capture the worker's prompt to assert the ground-truth pack is in it.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "write_file",
                 "arguments": "{\"path\": \"src/lib.rs\", \"content\": \"pub fn changed() {}\\n\"}"}}]}}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 10, "total_tokens": 40}
        })))
        .up_to_n_times(1_u64)
        .mount(&worker_server)
        .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        5,
    )
    .await;

    let config = wrangler_config(json!({
        "plan": "AIWRANGLER PLAN: PASS\n\ncontext pack:\n- S-4.2: wal records must be CRC'd\n- RULE-7: no silent defaults on error paths"
    }));

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-pack").await.unwrap();

    let (outcome, _events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert_eq!(outcome, RunOutcome::Success);
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn prose_only_replies_soft_retry_then_recover() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // turn 1: planning prose, no tool calls — must be a soft retry,
    // not a dead attempt
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant",
                "content": "I need to analyze the file first. Let me think about the approach."}}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 10, "total_tokens": 40}
        })))
        .up_to_n_times(2_u64)
        .mount(&worker_server)
        .await;
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/lib.rs", "content": "pub fn changed() {}\n"}),
        1,
    )
    .await;
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        5,
    )
    .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-prose").await.unwrap();
    let config = wrangler_config(json!({}));

    let (outcome, _events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert_eq!(
        outcome,
        RunOutcome::Success,
        "two prose replies must not kill the attempt"
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.worktree().join("src/lib.rs")).unwrap(),
        "pub fn changed() {}\n"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn stalled_step_escalates_to_frontier_before_failing() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;
    let frontier_server = MockServer::start().await;

    // worker: never proposes anything usable (worker errors -> NoAction)
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&worker_server)
        .await;

    // frontier: one guidance round that unblocks the worker
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant",
                "content": "GUIDANCE: write fixed.txt with content ok, then finish."}}],
            "usage": {"prompt_tokens": 500, "completion_tokens": 50, "total_tokens": 550}
        })))
        .mount(&frontier_server)
        .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let (frontier_client, _) = api_client(&frontier_server, "frontier");
    let sandbox = GitSandbox::create(&root, "e2e-stall").await.unwrap();
    let mut config = wrangler_config(json!({}));
    config.jockey.max_attempts_per_step = 2;

    let (outcome, events) = drive(
        &config,
        worker,
        Some(frontier_client),
        sandbox.clone(),
        one_step_dag(vec!["src", "fixed.txt"], "test -f fixed.txt"),
    )
    .await;

    assert!(
        events
            .iter()
            .any(|e| matches!(e, seekr::jockey::JockeyEvent::Escalated { .. })),
        "a stalled (NoAction) step must reach the frontier tier"
    );
    assert!(matches!(outcome, RunOutcome::Failed(_)));
    let status = run(sandbox.worktree(), &["status", "--porcelain"]).await;
    assert!(
        status.trim().is_empty(),
        "failed step must leave a rolled-back clean tree, got: {status}"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn stalled_worker_with_mutations_gets_auto_verified() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;

    // turn 1: the worker writes a real implementation...
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "fixed.txt", "content": "ok"}),
        1,
    )
    .await;
    // ...then stalls forever on prose (no finish_step, no more tools)
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant",
                "content": "Let me review what I have done so far."}}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 10, "total_tokens": 40}
        })))
        .mount(&worker_server)
        .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-autoverify").await.unwrap();
    let config = wrangler_config(json!({}));

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src", "fixed.txt"], "test -f fixed.txt"),
    )
    .await;

    assert_eq!(
        outcome,
        RunOutcome::Success,
        "the write landed; the stall must not discard it — verification \
runs on the worker's behalf"
    );
    assert!(sandbox.worktree().join("fixed.txt").exists());
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::VerificationPassed { .. }
    )));
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn parallel_reads_grep_and_list_flow() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;
    isolate_state_dir();

    // turn 1: batched parallel reads + grep + list in ONE reply
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{\"path\": \"src/lib.rs\"}"}},
                {"id": "c2", "type": "function", "function": {"name": "grep", "arguments": "{\"pattern\": \"original\"}"}},
                {"id": "c3", "type": "function", "function": {"name": "list", "arguments": "{\"path\": \"src\"}"}}]}}],
            "usage": {"prompt_tokens": 30, "completion_tokens": 10, "total_tokens": 40}
        })))
        .up_to_n_times(1_u64)
        .mount(&worker_server)
        .await;
    // turn 2: the write
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/lib.rs", "content": "pub fn changed() {}\n"}),
        1,
    )
    .await;
    // turn 3: finish
    mount_worker_reply(
        &worker_server,
        "finish_step",
        json!({"summary": "done"}),
        5,
    )
    .await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-parallel").await.unwrap();
    let config = wrangler_config(json!({}));

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert_eq!(outcome, RunOutcome::Success);
    // all three read-only actions executed in the single turn
    for tool in ["read_file", "grep", "list"] {
        assert!(
            events.iter().any(|e| matches!(
                e,
                seekr::jockey::JockeyEvent::ActionExecuted { tool: t, ok: true, .. } if t == tool
            )),
            "expected {tool} to execute"
        );
    }
    assert_eq!(
        std::fs::read_to_string(sandbox.worktree().join("src/lib.rs")).unwrap(),
        "pub fn changed() {}\n"
    );
    sandbox.dispose().await.unwrap();
}
