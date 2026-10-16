//! Live worker probe: drives one bounded-context worker against a real
//! OpenAI-compatible endpoint and reports whether it ever proposes a
//! mutating action (write_file / edit_file) or reaches finish_step.
//!
//! Env:
//!   SEEKR_PROBE_REPO   repo to point the sandbox at (required)
//!   SEEKR_PROBE_URL    endpoint, default http://server:11434/v1
//!   SEEKR_PROBE_MODEL  model id, default qwen3.5:latest

use seekr::jockey::tools::ToolCtx;
use seekr::jockey::worker::{Worker, WorkerAction, WorkerPrompt};
use std::collections::HashMap;

#[tokio::main]
async fn main() {
    let repo = std::path::PathBuf::from(
        std::env::var("SEEKR_PROBE_REPO")
            .expect("SEEKR_PROBE_REPO=<path> required"),
    );
    let base_url = std::env::var("SEEKR_PROBE_URL")
        .unwrap_or_else(|_| "http://server:11434/v1".into());
    let model = std::env::var("SEEKR_PROBE_MODEL")
        .unwrap_or_else(|_| "qwen3.5:latest".into());
    let provider = seekr::config::ProviderConfig {
        name: "probe".into(),
        key: String::new(),
        base_url,
        model: model.clone(),
        timeout: Some(300),
    };
    let config = seekr::config::AppConfig::default();
    let worker = Worker::new(
        seekr::api::client::ApiClient::new_for_provider(&config, &provider),
        model,
        0.2,
        8192,
        Some("none".into()),
    );
    let tool_ctx = ToolCtx {
        sandbox: seekr::sandbox::paths::PathSandbox::new(
            &repo,
            &[std::path::PathBuf::from("src")],
        )
        .unwrap(),
        worktree: repo.clone(),
        blocklist: config.agent.shell_blocklist.clone(),
        verification_timeout: 600,
        read_lines: 400,
        allow_web: false,
        cargo_envs: vec![],
    };

    let mut context: Vec<String> = vec![];
    let mut read_cache: HashMap<String, ()> = HashMap::new();
    for round in 1..=10 {
        let prompt = WorkerPrompt {
            goal: "Implement a crash-resilient WAL-backed storage engine with CRC verification and torn-write recovery.".into(),
            step_id: "wal_writer".into(),
            step_description: "Implement WalWriter serialization, CRC32 calculation, and append logic".into(),
            invariants: vec![
                "Record format: 4-byte CRC32 LE, 4-byte key_len LE, 4-byte val_len LE (0xFFFFFFFF for tombstone), key bytes, val bytes".into(),
                "WalWriter::open must open in append mode or create if missing".into(),
                "Must touch only src/lib.rs".into(),
                "cargo check --lib must exit 0".into(),
            ],
            allowed_paths: vec!["src/".into()],
            attempt: 1,
            max_attempts: 3,
            context: context.clone(),
            todos: vec![],
            tools_enabled: vec![],
        };
        let turn = match worker.propose(&prompt).await {
            Ok(t) => t,
            Err(e) => {
                println!("round {round}: ERROR {e}");
                context.push(format!("ERROR: {e}"));
                continue;
            }
        };
        println!("round {round}: {} action(s)", turn.actions.len());
        let mut next_ctx = vec![];
        for action in turn.actions.iter().take(6) {
            let brief = seekr::jockey::interceptor::action_brief(action);
            println!("   -> {} {brief}", action.tool);
            if action.tool == "finish_step" {
                println!("FINISH_STEP reached");
                return;
            }
            if action.tool == "read_file"
                && let Some(p) =
                    action.args.get("path").and_then(|v| v.as_str())
                && read_cache.contains_key(p)
            {
                next_ctx.push(format!(
                    "ACTION (cached): read_file {p} unchanged; do something new"
                ));
                continue;
            }
            if matches!(
                action.tool.as_str(),
                "read_file"
                    | "grep"
                    | "glob"
                    | "list"
                    | "diagnostics"
                    | "write_file"
                    | "edit_file"
                    | "run_command"
                    | "web_fetch"
            ) {
                let (ok, result) = tool_ctx
                    .execute(&WorkerAction {
                        id: action.id.clone(),
                        tool: action.tool.clone(),
                        args: action.args.clone(),
                    })
                    .await;
                println!("      ok={ok} {} chars", result.len());
                next_ctx.push(format!(
                    "ACTION {}: {} -> {}",
                    turn.actions.len(),
                    brief,
                    &result[..result.len().min(3000)]
                ));
                if action.tool == "read_file"
                    && let Some(p) =
                        action.args.get("path").and_then(|v| v.as_str())
                {
                    read_cache.insert(p.to_string(), ());
                }
                if matches!(action.tool.as_str(), "write_file" | "edit_file")
                    && ok
                {
                    println!("MUTATION MADE — model can write");
                    return;
                }
            } else {
                next_ctx.push("ACTION (todo): updated".to_string());
            }
        }
        context.extend(next_ctx);
    }
    println!("probe finished without edit");
}
