pub mod client;

pub use client::{
    CheckAction, CheckVerdict, PlanAction, PlanVerdict, WranglerClient,
    WranglerError,
};

use std::io;
use std::path::Path;

/// How the worktree got an `aiwrangler.config.json` for this run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateConfigOrigin {
    /// The target repo already commits one; its ground truth governs.
    Committed,
    /// seekr generated a minimal one (default gates, state-dir ledger).
    Generated,
}

/// File name AiWrangler discovers from the workspace root upward.
pub const GATE_CONFIG_FILE: &str = "aiwrangler.config.json";

/// Guarantees the worktree has a discoverable gate config.
///
/// AiWrangler's config discovery walks up from the gate dir and its gate
/// root (where diffs are judged and state lives) is the config file's own
/// directory. Linked worktrees live *outside* the target repo, so a config
/// committed in the target repo is present in the worktree automatically;
/// when there is none, discovery would escape to unrelated parent dirs or
/// the user-global config with the wrong corpus — so we generate a minimal
/// one at the worktree root with ledger/cache redirected to the run state
/// dir. It stays untracked and is excluded from checkpoints via
/// `GitSandbox::CHECKPOINT_EXCLUDES` — note that git does not honor
/// per-worktree `info/exclude` for linked worktrees, so exclusion has to
/// happen at the `git add` pathspec layer, not via ignore rules.
pub fn ensure_gate_config(
    worktree: &Path,
    state_dir: &Path,
) -> io::Result<GateConfigOrigin> {
    let config_path = worktree.join(GATE_CONFIG_FILE);
    if config_path.is_file() {
        return Ok(GateConfigOrigin::Committed);
    }

    let wrangler_state = state_dir.join("wrangler");
    std::fs::create_dir_all(&wrangler_state)?;
    let config = serde_json::json!({
        "model": "jev-1.13.0",
        "cache": { "dir": wrangler_state.join("cache").display().to_string() },
        "ledger": { "path": wrangler_state.join("ledger.jsonl").display().to_string() },
    });
    std::fs::write(
        &config_path,
        serde_json::to_string_pretty(&config).unwrap_or_default(),
    )?;
    Ok(GateConfigOrigin::Generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_config_is_excluded_and_ledger_redirected() {
        let repo = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        // minimal repo shape: .git dir (plain repo) is enough for the test
        std::fs::create_dir_all(repo.path().join(".git")).unwrap();

        let origin = ensure_gate_config(repo.path(), state.path()).unwrap();
        assert_eq!(origin, GateConfigOrigin::Generated);
        let cfg = std::fs::read_to_string(repo.path().join(GATE_CONFIG_FILE))
            .unwrap();
        assert!(cfg.contains("jev-1.13.0"));
        assert!(cfg.contains("wrangler/cache"));

        // second call is a no-op returning Committed (file now exists)
        let origin2 = ensure_gate_config(repo.path(), state.path()).unwrap();
        assert_eq!(origin2, GateConfigOrigin::Committed);
    }
}
