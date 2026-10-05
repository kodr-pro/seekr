use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Per-tier cost accounting for one run.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CostLedger {
    pub worker_calls: u64,
    pub worker_prompt_tokens: u64,
    pub worker_completion_tokens: u64,
    pub jev_calls: u64,
    pub jev_input_tokens: u64,
    pub jev_output_tokens: u64,
    pub jev_cache_hits: u64,
    pub frontier_calls: u64,
    pub frontier_prompt_tokens: u64,
    pub frontier_completion_tokens: u64,
}

impl CostLedger {
    pub fn add_worker(&mut self, usage: Option<&crate::api::types::Usage>) {
        self.worker_calls += 1;
        if let Some(u) = usage {
            self.worker_prompt_tokens += u.prompt_tokens as u64;
            self.worker_completion_tokens += u.completion_tokens as u64;
        }
    }

    pub fn add_frontier(&mut self, usage: Option<&crate::api::types::Usage>) {
        self.frontier_calls += 1;
        if let Some(u) = usage {
            self.frontier_prompt_tokens += u.prompt_tokens as u64;
            self.frontier_completion_tokens += u.completion_tokens as u64;
        }
    }

    pub fn add_jev(&mut self, result: &crate::jev::JevResult) {
        self.add_jev_usage(result.cached, &result.usage);
    }

    pub fn add_jev_usage(
        &mut self,
        cached: bool,
        usage: &crate::jev::client::JevUsage,
    ) {
        if cached {
            self.jev_cache_hits += 1;
        } else {
            self.jev_calls += 1;
        }
        self.jev_input_tokens += usage.input_tokens;
        self.jev_output_tokens += usage.output_tokens;
    }
}

/// Structured events emitted by the governor; consumed by the TUI, the
/// headless printer, and persisted as JSONL.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JockeyEvent {
    RunStarted {
        run_id: String,
        goal: String,
    },
    PlanReady {
        summary: String,
    },
    StepStarted {
        step_id: String,
        attempt: u32,
    },
    ActionProposed {
        step_id: String,
        tool: String,
        brief: String,
        #[serde(default)]
        args: Option<serde_json::Value>,
    },
    ActionApproved {
        tool: String,
        scope_p: Option<f64>,
        novelty: Option<f64>,
    },
    ActionRejected {
        tool: String,
        reason: String,
    },
    ActionExecuted {
        tool: String,
        ok: bool,
        brief: String,
    },
    VerificationStarted {
        step_id: String,
        command: String,
    },
    VerificationPassed {
        step_id: String,
        commit: String,
        duration_secs: f64,
    },
    VerificationFailed {
        step_id: String,
        output: String,
    },
    RolledBack {
        step_id: String,
        to_commit: String,
    },
    Triage {
        step_id: String,
        verdict: String,
        confidence: f64,
    },
    Escalated {
        step_id: String,
        reason: String,
    },
    StepCompleted {
        step_id: String,
        commit: String,
    },
    StepFailed {
        step_id: String,
        reason: String,
    },
    FrontierGuidance {
        step_id: String,
        brief: String,
    },
    Usage {
        ledger: CostLedger,
    },
    RunCompleted {
        status: String,
        ledger: CostLedger,
        branch: String,
        worktree: PathBuf,
    },
    RunFailed {
        reason: String,
        ledger: CostLedger,
    },
}

/// Append-only JSONL event log for a run.
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    pub fn create(dir: &std::path::Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            path: dir.join("events.jsonl"),
        })
    }

    pub fn append(&self, event: &JockeyEvent) {
        if let Ok(line) = serde_json::to_string(event) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                let _ = writeln!(f, "{line}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_accumulates_per_tier() {
        let mut l = CostLedger::default();
        l.add_worker(Some(&crate::api::types::Usage {
            prompt_tokens: 100,
            completion_tokens: 20,
            total_tokens: 120,
        }));
        l.add_frontier(None);
        assert_eq!(l.worker_prompt_tokens, 100);
        assert_eq!(l.frontier_calls, 1);

        let mut result = crate::jev::JevResult {
            model: "jev-1.13.0".into(),
            answers: Default::default(),
            usage: crate::jev::JevUsage {
                input_tokens: 300,
                output_tokens: 21,
            },
            cached: false,
        };
        l.add_jev(&result);
        result.cached = true;
        l.add_jev(&result);
        assert_eq!(l.jev_calls, 1);
        assert_eq!(l.jev_cache_hits, 1);
        assert_eq!(l.jev_input_tokens, 600);
    }

    #[test]
    fn events_serialize_tagged() {
        let e = JockeyEvent::ActionRejected {
            tool: "write_file".into(),
            reason: "out of scope".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""type":"action_rejected""#));
        assert!(s.contains("out of scope"));
    }

    #[test]
    fn event_log_appends_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::create(dir.path()).unwrap();
        log.append(&JockeyEvent::RunStarted {
            run_id: "r1".into(),
            goal: "g".into(),
        });
        log.append(&JockeyEvent::StepStarted {
            step_id: "s1".into(),
            attempt: 1,
        });
        let content =
            std::fs::read_to_string(dir.path().join("events.jsonl")).unwrap();
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains("run_started"));
    }
}
