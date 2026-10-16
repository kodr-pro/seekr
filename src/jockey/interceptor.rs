//! Phase 2 interceptor: deterministic, fail-closed, and local.
//!
//! Every tool that cannot mutate the workspace is free. Mutating actions
//! (write_file / edit_file / non-readonly run_command) pass three
//! deterministic gates before execution: the path/command sandbox verdict,
//! duplicate-failure detection (loop breaker), and the read-only command
//! allowlist. Semantic judgment — scope discipline, injection defense,
//! claims-vs-diff, silent defaults — is delegated to AiWrangler's
//! `wrangler_check` battery when the step completes (see
//! `crate::wrangler`), which gates the checkpoint commit the same way the
//! per-action Jev call used to gate the write.

use sha2::{Digest, Sha256};

use crate::jockey::worker::WorkerAction;

/// Fingerprinted summary of one failed attempt, used for loop detection.
#[derive(Clone, Debug)]
pub struct AttemptRecord {
    pub fingerprint: String,
    pub brief: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InterceptVerdict {
    Approved,
    Rejected { reason: String },
}

/// Deterministic fingerprint of an action: tool + canonical args
/// (key order insensitive).
pub fn fingerprint(action: &WorkerAction) -> String {
    let canonical = serde_json::json!({
        "tool": action.tool,
        "args": action.args,
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    format!("{:x}", Sha256::digest(&bytes))[..16].to_string()
}

/// True for every tool that cannot mutate the workspace: reads, searches,
/// listings, diagnostics, todos, web fetches, finish. These execute
/// without further review.
pub fn is_free_action(tool: &str) -> bool {
    matches!(
        tool,
        crate::jockey::worker::TOOL_READ_FILE
            | crate::jockey::worker::TOOL_GREP
            | crate::jockey::worker::TOOL_GLOB
            | crate::jockey::worker::TOOL_LIST
            | crate::jockey::worker::TOOL_TODO
            | crate::jockey::worker::TOOL_DIAGNOSTICS
            | crate::jockey::worker::TOOL_WEB_FETCH
            | crate::jockey::worker::TOOL_FINISH_STEP
    )
}

/// Deterministic review of one proposed action. `deterministic_error`
/// carries the sandbox verdict (path containment or shell blocklist);
/// `failed` carries fingerprints of this step's prior failed attempts.
pub fn review(
    action: &WorkerAction,
    failed: &[AttemptRecord],
    deterministic_error: Option<String>,
) -> InterceptVerdict {
    if let Some(err) = deterministic_error {
        return InterceptVerdict::Rejected { reason: err };
    }
    if is_free_action(&action.tool) {
        return InterceptVerdict::Approved;
    }
    if failed.iter().any(|r| r.fingerprint == fingerprint(action)) {
        return InterceptVerdict::Rejected {
            reason: "loop detected: this exact action already failed in a \
previous attempt of this step"
                .to_string(),
        };
    }
    InterceptVerdict::Approved
}

/// Human-readable one-liner for events and summaries.
pub fn action_brief(action: &WorkerAction) -> String {
    match action.tool.as_str() {
        t if t == crate::jockey::worker::TOOL_READ_FILE
            || t == crate::jockey::worker::TOOL_WRITE_FILE
            || t == crate::jockey::worker::TOOL_EDIT_FILE =>
        {
            action
                .args
                .get("path")
                .and_then(|p| p.as_str())
                .map(|p| format!("{} {p}", action.tool))
                .unwrap_or_else(|| action.tool.clone())
        }
        t if t == crate::jockey::worker::TOOL_RUN_COMMAND => action
            .args
            .get("command")
            .and_then(|c| c.as_str())
            .map(|c| {
                let brief: String = c.chars().take(120).collect();
                format!("run `{brief}`")
            })
            .unwrap_or_else(|| action.tool.clone()),
        _ => action.tool.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jockey::worker::{
        TOOL_EDIT_FILE, TOOL_READ_FILE, TOOL_WRITE_FILE,
    };
    use serde_json::json;

    fn action(tool: &str, args: serde_json::Value) -> WorkerAction {
        WorkerAction {
            id: "c1".into(),
            tool: tool.into(),
            args,
        }
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let a = action(
            TOOL_WRITE_FILE,
            json!({"path": "src/a.rs", "content": "x"}),
        );
        let a2 = action(
            TOOL_WRITE_FILE,
            json!({"content": "x", "path": "src/a.rs"}),
        );
        assert_eq!(fingerprint(&a), fingerprint(&a2));
        let b = action(
            TOOL_WRITE_FILE,
            json!({"path": "src/b.rs", "content": "x"}),
        );
        assert_ne!(fingerprint(&a), fingerprint(&b));
    }

    #[test]
    fn deterministic_error_rejects() {
        let v = review(
            &action(
                TOOL_WRITE_FILE,
                json!({"path": "etc/passwd", "content": "x"}),
            ),
            &[],
            Some("path outside allowed_paths: etc/passwd".into()),
        );
        assert!(matches!(v, InterceptVerdict::Rejected { .. }));
    }

    #[test]
    fn read_tools_are_free() {
        let v = review(
            &action(TOOL_READ_FILE, json!({"path": "src/a.rs"})),
            &[],
            None,
        );
        assert_eq!(v, InterceptVerdict::Approved);
    }

    #[test]
    fn duplicate_failed_action_is_rejected() {
        let a = action(
            TOOL_WRITE_FILE,
            json!({"path": "src/a.rs", "content": "x"}),
        );
        let failed = vec![AttemptRecord {
            fingerprint: fingerprint(&a),
            brief: "write_file src/a.rs".into(),
        }];
        let v = review(&a, &failed, None);
        assert!(
            matches!(v, InterceptVerdict::Rejected { reason } if reason.contains("loop"))
        );
    }

    #[test]
    fn fresh_mutation_is_approved() {
        let v = review(
            &action(
                TOOL_EDIT_FILE,
                json!({"path": "src/a.rs", "old_string": "a", "new_string": "b"}),
            ),
            &[],
            None,
        );
        assert_eq!(v, InterceptVerdict::Approved);
    }
}
