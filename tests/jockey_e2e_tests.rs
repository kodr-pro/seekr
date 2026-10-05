use std::path::Path;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use seekr::api::client::ApiClient;
use seekr::config::{AppConfig, JockeyConfig, ProviderConfig};
use seekr::jev::client::{JevConfig, JevClient};
use seekr::jockey::dag::{DagStep, StepStatus, TaskDag};
use seekr::jockey::driver::{Governor, RunOutcome};
use seekr::jockey::worker::Worker;
use seekr::sandbox::git::GitSandbox;

async fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    run(&root, &["init", "--quiet"]).await;
    run(&root, &["config", "user.email", "jj@test"]).await;
    run(&root, &["config", "user.name", "jj"]).await;
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn original() {}\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
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

fn jev_client(server: &MockServer) -> JevClient {
    let cfg = JevConfig {
        base_url: server.uri(),
        api_key: "ts-test".into(),
        model: "jev-test".into(),
        egress_enabled: true,
        timeout: std::time::Duration::from_secs(5),
        max_retries: 0,
        cache_dir: None,
    };
    JevClient::new(cfg)
}

fn tool_call(name: &str, args: serde_json::Value) -> serde_json::Value {
    json!([{
        "id": format!("call_{name}"),
        "type": "function",
        "function": { "name": name, "arguments": args.to_string() }
    }])
}

async fn mount_worker_reply(server: &MockServer, tool: &str, args: serde_json::Value, times: usize) {
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

async fn mount_jev_superset(server: &MockServer, scope: f64, triage: &str) {
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-test",
            "answers": {
                "scope": {"type": "noul", "noul": scope},
                "steering": {"type": "noul", "noul": 0.01},
                "novelty": {"type": "score", "score": 3.0, "confidence": 0.8,
                            "legend": {"0": "identical", "1": "trivial", "2": "partial", "3": "novel"},
                            "probabilities": {"0": 0.05, "1": 0.05, "2": 0.1, "3": 0.8}},
                "triage": {"type": "choice", "choice": triage, "confidence": 0.9,
                           "probabilities": {"syntax_fix": 0.9, "read_context": 0.05, "deadlock": 0.05}}
            },
            "usage": {"input_tokens": 100, "output_tokens": 10}
        })))
        .mount(server)
        .await;
}

async fn mount_jev_triage(server: &MockServer, verdict: &str) {
    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "model": "jev-test",
            "answers": {
                "triage": {"type": "choice", "choice": verdict, "confidence": 0.9,
                           "probabilities": {"syntax_fix": 0.9, "read_context": 0.05, "deadlock": 0.05}}
            },
            "usage": {"input_tokens": 100, "output_tokens": 10}
        })))
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
            allowed_paths: allowed.into_iter().map(std::path::PathBuf::from).collect(),
            verification_command: Some(verify.into()),
            status: StepStatus::Pending,
        }],
    }
}

async fn drive(
    config: &AppConfig,
    worker: Worker,
    frontier: Option<ApiClient>,
    jev: JevClient,
    sandbox: GitSandbox,
    dag: TaskDag,
) -> (RunOutcome, Vec<seekr::jockey::JockeyEvent>) {
    let frontier = frontier.map(|c| (c, "mock-model".to_string()));
    let run_id = format!("test-{}", uuid::Uuid::new_v4().simple());
    let mut governor = Governor::new(config, worker, frontier, jev, sandbox, "test goal".into(), dag, run_id).unwrap();
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
    let jev_server = MockServer::start().await;

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
    mount_worker_reply(&worker_server, "finish_step", json!({"summary": "done"}), 5).await;
    mount_jev_superset(&jev_server, 0.95, "syntax_fix").await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-a").await.unwrap();
    let config = AppConfig::default();

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        jev_client(&jev_server),
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
    assert!(status.trim().is_empty(), "checkpoint must leave a clean tree");
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn verification_failure_retries_with_loop_rejection_then_recovers() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;
    let jev_server = MockServer::start().await;

    // attempt 1: writes a file, finishes -> verification fails (no marker)
    mount_worker_reply(
        &worker_server,
        "write_file",
        json!({"path": "src/attempt1.rs", "content": "first"}),
        1,
    )
    .await;
    mount_worker_reply(&worker_server, "finish_step", json!({"summary": "done"}), 1).await;
    // attempt 2: proposes the IDENTICAL write again -> fingerprint loop reject,
    // then a different action that satisfies verification
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
    mount_worker_reply(&worker_server, "finish_step", json!({"summary": "fixed"}), 5).await;

    mount_jev_superset(&jev_server, 0.95, "syntax_fix").await;

    let (client, model) = api_client(&worker_server, "worker");
    let worker = Worker::new(client, model, 0.2, 1024, None);
    let sandbox = GitSandbox::create(&root, "e2e-b").await.unwrap();
    let mut config = AppConfig::default();
    config.jockey = JockeyConfig {
        max_attempts_per_step: 3,
        ..Default::default()
    };

    let (outcome, events) = drive(
        &config,
        worker,
        None,
        jev_client(&jev_server),
        sandbox.clone(),
        one_step_dag(vec!["src", "fixed.txt"], "test -f fixed.txt"),
    )
    .await;

    assert_eq!(outcome, RunOutcome::Success);
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::VerificationFailed { .. }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::RolledBack { .. }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::ActionRejected { reason, .. } if reason.contains("loop detected")
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::Triage { verdict, .. } if verdict == "syntax_fix"
    )));
    assert!(sandbox.worktree().join("fixed.txt").exists());
    assert!(
        !sandbox.worktree().join("src/attempt1.rs").exists(),
        "failed attempt's files must be rolled back"
    );
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn exhausted_attempts_escalate_and_fail_cleanly() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;
    let frontier_server = MockServer::start().await;
    let jev_server = MockServer::start().await;

    // worker: declares the step done without ever fixing it
    mount_worker_reply(&worker_server, "finish_step", json!({"summary": "done"}), 100).await;
    mount_jev_triage(&jev_server, "syntax_fix").await;

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
    let mut config = AppConfig::default();
    config.jockey = JockeyConfig {
        max_attempts_per_step: 2,
        ..Default::default()
    };

    let (outcome, events) = drive(
        &config,
        worker,
        Some(frontier_client),
        jev_client(&jev_server),
        sandbox.clone(),
        one_step_dag(vec!["src"], "test -f fixed.txt"),
    )
    .await;

    assert!(matches!(outcome, RunOutcome::Failed(_)));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::Escalated { .. }
    )));
    assert!(events.iter().any(|e| matches!(
        e,
        seekr::jockey::JockeyEvent::FrontierGuidance { .. }
    )));
    assert!(
        !sandbox.worktree().join("fixed.txt").exists(),
        "nothing may persist after a failed run"
    );
    let status = run(sandbox.worktree(), &["status", "--porcelain"]).await;
    assert!(status.trim().is_empty(), "worktree must be rolled back clean");
    sandbox.dispose().await.unwrap();
}

#[tokio::test]
async fn fail_closed_when_jev_unavailable_blocks_writes() {
    let (_tmp, root) = init_repo().await;
    let worker_server = MockServer::start().await;
    let dead_jev = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/systemone"))
        .respond_with(ResponseTemplate::new(500).set_body_string("down"))
        .mount(&dead_jev)
        .await;

    // worker: keeps proposing an in-scope write; every proposal must be rejected
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

    let (outcome, _events) = drive(
        &AppConfig::default(),
        worker,
        None,
        jev_client(&dead_jev),
        sandbox.clone(),
        one_step_dag(vec!["src"], "true"),
    )
    .await;

    assert!(matches!(outcome, RunOutcome::Failed(_)));
    assert_eq!(
        std::fs::read_to_string(sandbox.worktree().join("src/lib.rs")).unwrap(),
        "pub fn original() {}\n",
        "fail-closed: no write may land while Jev is down"
    );
    sandbox.dispose().await.unwrap();
}
