//! Hermetic mock of the `aiwrangler-mcp` stdio MCP server for seekr's
//! test suite. Speaks just enough JSON-RPC NDJSON: `initialize`,
//! `notifications/initialized`, and `tools/call` for the wrangler_*
//! tools. Canned responses come from a scenario file passed as the first
//! argument so parallel tests never race on process environment:
//!
//! ```json
//! {
//!   "plan":  "AIWRANGLER PLAN: PASS\n\ncontext pack:\n- S1: ground truth",
//!   "checks": ["AIWRANGLER FIX REQUIRED (exit 17)\n...", "AIWRANGLER PASS (exit 0)"],
//!   "die_on_check": false,
//!   "hang_after_init": false
//! }
//! ```
//!
//! `checks` are served sequentially; the last repeats. Missing fields
//! default to healthy PASS responses. `die_on_check` exits the process
//! when the first `wrangler_check` arrives (simulates a dead server
//! mid-run). `hang_after_init` completes the handshake and then never
//! answers tool calls (simulates a timeout).
//!
//! Run directly (no arguments) it serves a healthy PASS scenario — handy
//! for smoke-testing a seekr install without spending judgment tokens.

use std::io::{BufRead, Write};

fn main() {
    let scenario = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(serde_json::json!({}));

    let plan_text = scenario["plan"].as_str().map(str::to_string).unwrap_or_else(|| {
        "AIWRANGLER PLAN: PASS\n\nreasons:\n  - task is in spec\n\ncontext pack:\n- S1: honor the invariants\n- S2: tests must actually assert".to_string()
    });
    let checks: Vec<String> = scenario["checks"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let default_check =
        "AIWRANGLER PASS (exit 0)\ndirty: 1 file(s)\nsemantic verdicts (jev, 100 in / 20 out tokens):"
            .to_string();
    let die_on_check = scenario["die_on_check"].as_bool().unwrap_or(false);
    let hang = scenario["hang_after_init"].as_bool().unwrap_or(false);

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut check_idx: usize = 0;

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(id) = msg["id"].clone().as_i64() else {
            continue; // notification — nothing to answer
        };
        let method = msg["method"].as_str().unwrap_or_default();

        if hang && method == "tools/call" {
            // Simulate a wedged server: acknowledge nothing, sleep forever.
            std::thread::sleep(std::time::Duration::from_secs(600));
            return;
        }

        let text: Option<String> = match method {
            "initialize" => None, // handled specially below
            "tools/call" => {
                let name = msg["params"]["name"].as_str().unwrap_or_default();
                match name {
                    "wrangler_plan" => Some(plan_text.clone()),
                    "wrangler_check" => {
                        if die_on_check {
                            std::process::exit(1);
                        }
                        let t = checks
                            .get(check_idx)
                            .cloned()
                            .unwrap_or_else(|| default_check.clone());
                        check_idx += 1;
                        Some(t)
                    }
                    "wrangler_config" => Some(
                        "config: (scenario)\nmodel: jev-test  policy: strict\negress: semantic=ON"
                            .to_string(),
                    ),
                    "wrangler_ack" => {
                        Some("no pending human-severity findings".to_string())
                    }
                    "wrangler_loop_reset" => {
                        Some("loop state cleared".to_string())
                    }
                    _ => Some(format!("unknown tool {name}")),
                }
            }
            _ => None,
        };

        let response = if method == "initialize" {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "mock-aiwrangler", "version": "0.1.0" }
                }
            })
        } else if let Some(t) = text {
            // Emit a stray notification first: clients must skip these.
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/progress",
                "params": { "progress": 1 }
            });
            let _ = writeln!(stdout, "{notification}");
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": t }],
                    "isError": false
                }
            })
        } else {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") }
            })
        };
        let _ = writeln!(stdout, "{response}");
        let _ = stdout.flush();
    }
}
