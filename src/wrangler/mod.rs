pub mod client;

pub use client::{
    CheckAction, CheckVerdict, PlanAction, PlanVerdict, WranglerClient,
    WranglerError,
};

use std::io;
use std::path::{Path, PathBuf};

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
/// dir, and git-exclude it so it never lands in diffs or on the branch.
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
    exclude_in_worktree(worktree, GATE_CONFIG_FILE)?;
    Ok(GateConfigOrigin::Generated)
}

/// Appends `entry` to the worktree-local git exclude so untracked helper
/// files stay out of diffs, status, and commits. Linked worktrees keep
/// their exclude at `<main>/.git/worktrees/<name>/info/exclude`, resolved
/// through the `.git` pointer file.
fn exclude_in_worktree(worktree: &Path, entry: &str) -> io::Result<()> {
    let dot_git = worktree.join(".git");
    let git_dir = if dot_git.is_file() {
        let raw = std::fs::read_to_string(&dot_git)?;
        raw.trim()
            .strip_prefix("gitdir:")
            .map(|s| s.trim().to_string())
            .map(PathBuf::from)
            .ok_or_else(|| {
                io::Error::other("unexpected .git pointer in worktree")
            })?
    } else {
        dot_git
    };
    let info_dir = git_dir.join("info");
    std::fs::create_dir_all(&info_dir)?;
    let exclude = info_dir.join("exclude");
    let mut existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == entry) {
        if !existing.ends_with('\n') && !existing.is_empty() {
            existing.push('\n');
        }
        existing.push_str(&format!("# added by seekr\n{entry}\n"));
        std::fs::write(&exclude, existing)?;
    }
    Ok(())
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
        let exclude =
            std::fs::read_to_string(repo.path().join(".git/info/exclude"))
                .unwrap();
        assert!(exclude.contains(GATE_CONFIG_FILE));

        // second call is a no-op returning Committed (file now exists)
        let origin2 = ensure_gate_config(repo.path(), state.path()).unwrap();
        assert_eq!(origin2, GateConfigOrigin::Committed);
    }
}
