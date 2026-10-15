//! stdio MCP client for AiWrangler (github.com/kodr-pro/AiWrangler).
//!
//! seekr spawns `aiwrangler-mcp` once per run and drives its gate
//! lifecycle: `wrangler_config` (health), `wrangler_plan` (pre-flight),
//! `wrangler_check` (step completion), `wrangler_ack` / `wrangler_loop_reset`
//! (owner controls). Reports are plain text with a stable first-line
//! contract (`AIWRANGLER PLAN: PASS|CLARIFY|HUMAN`, `AIWRANGLER PASS|FIX
//! REQUIRED|HUMAN REVIEW|DEGRADED (exit N)`) which we parse into typed
//! verdicts. Every failure mode funnels into a `*Verdict` with an explicit
//! action — transport failures surface as `Unreachable`, and the governor
//! treats that fail-closed exactly like `Degraded`.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::timeout;

const PROTOCOL_VERSION: &str = "2024-11-05";
const CLIENT_NAME: &str = "seekr";

#[derive(Debug, thiserror::Error)]
pub enum WranglerError {
    #[error("wrangler command not configured")]
    NoCommand,
    #[error("wrangler spawn failed: {0}")]
    Spawn(String),
    #[error("wrangler transport: {0}")]
    Transport(String),
    #[error("wrangler call timed out after {}s", _0.as_secs())]
    Timeout(Duration),
    #[error("wrangler tool error: {0}")]
    Tool(String),
}

/// Outcome of the pre-flight plan gate.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanVerdict {
    pub action: PlanAction,
    /// Set when the semantic layer was skipped (no key / egress off /
    /// API down) — deterministic verdicts only.
    pub degraded: Option<String>,
    /// Full report text (verdicts, reasons, context pack).
    pub raw: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanAction {
    /// Task is in spec; proceed.
    Pass,
    /// Ambiguous / unresolved question; the owner must answer first.
    Clarify,
    /// One-way door or steering risk; stop for the owner.
    Human,
    /// No gate config discoverable (seekr bootstraps one, so this is
    /// unexpected) — fail closed.
    Inactive,
    /// Gate errored or the server was unreachable — fail closed.
    Unreachable,
}

/// Outcome of the post-generation check gate.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckVerdict {
    pub action: CheckAction,
    pub exit_code: i64,
    /// Parsed semantic-battery token usage (`N in / M out`), when a
    /// semantic call ran.
    pub usage: Option<(u64, u64)>,
    /// Retry circuit-breaker state from the report.
    pub breaker_open: bool,
    /// Loop cycle count (1 = first occurrence).
    pub loop_cycle: u32,
    /// Full report text (findings, verdicts, reasons).
    pub raw: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckAction {
    /// All gates green.
    Pass,
    /// Cited, actionable findings; fix and re-run.
    FixRequired,
    /// Human-severity finding; the owner must review and `ack`.
    HumanReview,
    /// Semantic layer unavailable; deterministic findings only.
    Degraded,
    /// No gate config discoverable — unexpected under seekr; fail closed.
    Inactive,
    /// Server unreachable / transport failure — fail closed.
    Unreachable,
}

/// A live `aiwrangler-mcp` subprocess speaking MCP over stdio.
pub struct WranglerClient {
    command: Vec<String>,
    timeout: Duration,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    next_id: i64,
    /// Default gate dir passed as `dir` to every tool call.
    pub dir: PathBuf,
}

impl WranglerClient {
    /// Builds a client for `command` (argv split on whitespace) operating
    /// on gate dir `dir`. Nothing is spawned until [`connect`](Self::connect).
    pub fn new(
        command: &str,
        dir: impl Into<PathBuf>,
        timeout: Duration,
    ) -> Self {
        let argv: Vec<String> =
            command.split_whitespace().map(str::to_string).collect();
        Self {
            command: argv,
            timeout,
            child: None,
            stdin: None,
            stdout: None,
            next_id: 0,
            dir: dir.into(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.child.is_some()
    }

    /// Spawns the server and completes the MCP handshake.
    pub async fn connect(&mut self) -> Result<(), WranglerError> {
        self.kill().await;
        if self.command.is_empty() {
            return Err(WranglerError::NoCommand);
        }
        let mut cmd = tokio::process::Command::new(&self.command[0]);
        cmd.args(&self.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Per-call `dir` selects the workspace; keep the environment
            // (TYPESAFE_API_KEY, AIWRANGLER_EGRESS, ...) intact.
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| {
            WranglerError::Spawn(format!("{}: {e}", self.command[0]))
        })?;
        let mut child = child;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| WranglerError::Spawn("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| WranglerError::Spawn("no stdout".into()))?;
        self.stdin = Some(stdin);
        self.stdout = Some(BufReader::new(stdout));
        self.child = Some(child);

        let init = json!({
            "jsonrpc": "2.0",
            "id": self.alloc_id(),
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
            },
        });
        let _resp = self.roundtrip(init).await?;
        self.notify(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }))
        .await?;
        Ok(())
    }

    async fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
        self.stdin = None;
        self.stdout = None;
    }

    fn alloc_id(&mut self) -> i64 {
        self.next_id += 1;
        self.next_id
    }

    async fn notify(&mut self, msg: Value) -> Result<(), WranglerError> {
        let mut stdin = self
            .stdin
            .take()
            .ok_or_else(|| WranglerError::Transport("not connected".into()))?;
        let res = async {
            let line = serde_json::to_string(&msg)
                .map_err(|e| WranglerError::Transport(e.to_string()))?;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| WranglerError::Transport(e.to_string()))?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| WranglerError::Transport(e.to_string()))?;
            stdin
                .flush()
                .await
                .map_err(|e| WranglerError::Transport(e.to_string()))
        }
        .await;
        self.stdin = Some(stdin);
        res
    }

    /// Sends one request and awaits its matching response, skipping any
    /// server-initiated notifications along the way.
    async fn roundtrip(
        &mut self,
        request: Value,
    ) -> Result<Value, WranglerError> {
        let id = request["id"].as_i64();
        self.notify(request).await?;
        let deadline = self.timeout;
        let read = timeout(deadline, async {
            let stdout = self.stdout.take().ok_or_else(|| {
                WranglerError::Transport("not connected".into())
            })?;
            let mut stdout = stdout;
            let mut found: Option<Value> = None;
            let result: Result<(), WranglerError> = loop {
                let mut line = String::new();
                let n = stdout
                    .read_line(&mut line)
                    .await
                    .map_err(|e| WranglerError::Transport(e.to_string()))?;
                if n == 0 {
                    break Err(WranglerError::Transport(
                        "server closed stdout".into(),
                    ));
                }
                let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
                    continue; // stray non-JSON line
                };
                if msg.get("id").and_then(Value::as_i64) == id {
                    found = Some(msg);
                    break Ok(());
                }
                // notification or someone else's response: skip
            };
            self.stdout = Some(stdout);
            result.map(|()| found.expect("response captured"))
        })
        .await;
        match read {
            Ok(inner) => inner,
            Err(_) => {
                // A stuck call leaves the protocol stream desynchronized;
                // force a fresh process for the next attempt.
                self.kill().await;
                Err(WranglerError::Timeout(self.timeout))
            }
        }
    }

    /// Calls one wrangler tool and returns its text content.
    async fn call_tool(
        &mut self,
        name: &str,
        mut args: Value,
    ) -> Result<String, WranglerError> {
        if !self.is_connected() {
            self.connect().await?;
        }
        if !self.dir.as_os_str().is_empty() {
            args["dir"] = json!(self.dir.display().to_string());
        }
        let request = json!({
            "jsonrpc": "2.0",
            "id": self.alloc_id(),
            "method": "tools/call",
            "params": { "name": name, "arguments": args },
        });
        let response = self.roundtrip(request).await?;
        if let Some(err) = response.get("error") {
            return Err(WranglerError::Tool(err.to_string()));
        }
        let result = &response["result"];
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(WranglerError::Tool(extract_text(result)));
        }
        Ok(extract_text(result))
    }

    /// Health/config probe; also used as the connectivity check.
    pub async fn config_report(&mut self) -> Result<String, WranglerError> {
        self.call_tool("wrangler_config", json!({})).await
    }

    /// Pre-flight plan gate. Never fails — failures become
    /// `PlanAction::Unreachable` so callers stay fail-closed.
    pub async fn plan(
        &mut self,
        task: &str,
        plan: Option<&str>,
    ) -> PlanVerdict {
        let mut args = json!({ "task": task });
        if let Some(p) = plan {
            args["plan"] = json!(p);
        }
        match self.call_tool("wrangler_plan", args).await {
            Ok(text) => parse_plan(&text),
            Err(e) => PlanVerdict {
                action: PlanAction::Unreachable,
                degraded: None,
                raw: e.to_string(),
            },
        }
    }

    /// Post-generation check gate. Never fails — failures become
    /// `CheckAction::Unreachable` so callers stay fail-closed.
    pub async fn check(
        &mut self,
        task: Option<&str>,
        agent_summary: Option<&str>,
        tool_output: Option<&str>,
    ) -> CheckVerdict {
        let mut args = json!({});
        if let Some(t) = task {
            args["task"] = json!(t);
        }
        if let Some(s) = agent_summary {
            args["agent_summary"] = json!(s);
        }
        if let Some(o) = tool_output {
            args["tool_output"] = json!(o);
        }
        match self.call_tool("wrangler_check", args).await {
            Ok(text) => parse_check(&text),
            Err(e) => CheckVerdict {
                action: CheckAction::Unreachable,
                exit_code: -1,
                usage: None,
                breaker_open: false,
                loop_cycle: 1,
                raw: e.to_string(),
            },
        }
    }

    /// Records an owner acknowledgment (after explicit human approval).
    pub async fn ack(
        &mut self,
        rules: &[String],
        note: Option<&str>,
    ) -> Result<String, WranglerError> {
        let mut args = json!({ "rules": rules });
        if let Some(n) = note {
            args["note"] = json!(n);
        }
        self.call_tool("wrangler_ack", args).await
    }

    /// Lists pending human-severity findings (no-arg ack form).
    pub async fn pending_ack(&mut self) -> Result<String, WranglerError> {
        self.call_tool("wrangler_ack", json!({})).await
    }

    /// Resets the retry circuit breaker (owner decision).
    pub async fn loop_reset(&mut self) -> Result<String, WranglerError> {
        self.call_tool("wrangler_loop_reset", json!({})).await
    }
}

impl Drop for WranglerClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

fn extract_text(result: &Value) -> String {
    result["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| {
                    if p["type"] == "text" {
                        Some(p["text"].as_str().unwrap_or_default().to_string())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn parse_plan(text: &str) -> PlanVerdict {
    let first = text.lines().next().unwrap_or_default().trim();
    if first.contains("plan gate inactive") {
        return PlanVerdict {
            action: PlanAction::Inactive,
            degraded: None,
            raw: text.to_string(),
        };
    }
    if first.contains("plan gate error") {
        return PlanVerdict {
            action: PlanAction::Unreachable,
            degraded: None,
            raw: text.to_string(),
        };
    }
    let degraded = first.find("(degraded: ").map(|i| {
        let rest = &first[i + "(degraded: ".len()..];
        rest.trim_end_matches(')').to_string()
    });
    let action = if first.starts_with("AIWRANGLER PLAN: PASS") {
        PlanAction::Pass
    } else if first.starts_with("AIWRANGLER PLAN: CLARIFY") {
        PlanAction::Clarify
    } else if first.starts_with("AIWRANGLER PLAN: HUMAN") {
        PlanAction::Human
    } else {
        PlanAction::Unreachable
    };
    PlanVerdict {
        action,
        degraded,
        raw: text.to_string(),
    }
}

/// Leading unsigned integer of a string, ignoring trailing units like
/// " out tokens):".
fn leading_number(s: &str) -> Option<u64> {
    let digits: String =
        s.trim().chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn parse_check(text: &str) -> CheckVerdict {
    let first = text.lines().next().unwrap_or_default().trim();
    if first.contains("check gate inactive") {
        return CheckVerdict {
            action: CheckAction::Inactive,
            exit_code: -1,
            usage: None,
            breaker_open: false,
            loop_cycle: 1,
            raw: text.to_string(),
        };
    }
    if first.contains("check gate error") {
        return CheckVerdict {
            action: CheckAction::Unreachable,
            exit_code: -1,
            usage: None,
            breaker_open: false,
            loop_cycle: 1,
            raw: text.to_string(),
        };
    }
    let action = if first.starts_with("AIWRANGLER PASS") {
        CheckAction::Pass
    } else if first.starts_with("AIWRANGLER FIX REQUIRED") {
        CheckAction::FixRequired
    } else if first.starts_with("AIWRANGLER HUMAN REVIEW") {
        CheckAction::HumanReview
    } else if first.starts_with("AIWRANGLER DEGRADED") {
        CheckAction::Degraded
    } else {
        CheckAction::Unreachable
    };
    let exit_code = first
        .rsplit("(exit ")
        .next()
        .and_then(|s| s.trim_end_matches(')').parse().ok())
        .unwrap_or(-1);
    // semantic verdicts (jev, 1234 in / 56 out tokens):
    let usage = text.lines().find_map(|l| {
        let idx = l.find("(jev, ")?;
        let rest = &l[idx + "(jev, ".len()..];
        let mut parts = rest.split(" in / ");
        let input: u64 = leading_number(parts.next()?)?;
        let output: u64 = leading_number(parts.next()?)?;
        Some((input, output))
    });
    let breaker_open = text.contains("CIRCUIT BREAKER OPEN");
    let loop_cycle = text
        .lines()
        .find(|l| l.trim_start().starts_with("loop: cycle"))
        .and_then(|l| {
            leading_number(l.split("cycle").nth(1).unwrap_or_default())
        })
        .and_then(|c| u32::try_from(c).ok())
        .unwrap_or(1);
    CheckVerdict {
        action,
        exit_code,
        usage,
        breaker_open,
        loop_cycle,
        raw: text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan_verdicts() {
        for (text, action) in [
            ("AIWRANGLER PLAN: PASS\ncontext pack: ...", PlanAction::Pass),
            ("AIWRANGLER PLAN: CLARIFY\nreasons:", PlanAction::Clarify),
            ("AIWRANGLER PLAN: HUMAN\nSTOP:", PlanAction::Human),
        ] {
            assert_eq!(parse_plan(text).action, action, "{text}");
        }
        let degraded = parse_plan("AIWRANGLER PLAN: PASS (degraded: no key)");
        assert_eq!(degraded.action, PlanAction::Pass);
        assert_eq!(degraded.degraded.as_deref(), Some("no key"));
        assert_eq!(
            parse_plan("aiwrangler: no aiwrangler.config.json found for this directory; plan gate inactive").action,
            PlanAction::Inactive
        );
        assert_eq!(
            parse_plan("aiwrangler plan gate error: boom").action,
            PlanAction::Unreachable
        );
    }

    #[test]
    fn parses_check_verdicts() {
        let pass = parse_check("AIWRANGLER PASS (exit 0)\ndirty: 1 file(s)");
        assert_eq!(pass.action, CheckAction::Pass);
        assert_eq!(pass.exit_code, 0);

        let fix = parse_check(
            "AIWRANGLER FIX REQUIRED (exit 17)\n\nsemantic verdicts (jev, 1234 in / 56 out tokens):\ndeterministic findings:\n  RULE-1: thing\n\nloop: cycle 2",
        );
        assert_eq!(fix.action, CheckAction::FixRequired);
        assert_eq!(fix.exit_code, 17);
        assert_eq!(fix.usage, Some((1234, 56)));
        assert_eq!(fix.loop_cycle, 2);
        assert!(!fix.breaker_open);

        let breaker = parse_check(
            "AIWRANGLER FIX REQUIRED (exit 17)\nloop: cycle 3: CIRCUIT BREAKER OPEN",
        );
        assert!(breaker.breaker_open);
        assert_eq!(breaker.loop_cycle, 3);

        assert_eq!(
            parse_check("AIWRANGLER HUMAN REVIEW (exit 5)").action,
            CheckAction::HumanReview
        );
        assert_eq!(
            parse_check(
                "AIWRANGLER DEGRADED / UNVERIFIED (exit 3)\n\nDEGRADED: TYPESAFE_API_KEY unset: deterministic findings only; semantic layer unverified"
            )
            .action,
            CheckAction::Degraded
        );
        assert_eq!(
            parse_check("aiwrangler check gate error: boom").action,
            CheckAction::Unreachable
        );
    }
}
