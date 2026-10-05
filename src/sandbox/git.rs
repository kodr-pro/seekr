use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("not a git repository: {0}")]
    NotARepo(String),
    #[error("repository has tracked, uncommitted changes; commit or stash first: {0}")]
    DirtyRepo(String),
    #[error("git failed ({0}): {1}")]
    Git(&'static str, String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Isolated git worktree sandbox. The main checkout is never modified:
/// all work happens in a linked worktree on branch `jj/<task-id>`,
/// checkpoints are commits, and rollback is `reset --hard` + `clean -fd`
/// (which also removes untracked files the worker created).
#[derive(Clone, Debug)]
pub struct GitSandbox {
    repo_root: PathBuf,
    worktree: PathBuf,
    branch: String,
}

impl GitSandbox {
    /// Verifies the repo is a git repository without tracked modifications
    /// (untracked files in the main checkout are tolerated — they are
    /// invisible to the worktree).
    pub async fn require_clean_repo(repo_root: &Path) -> Result<(), SandboxError> {
        let status = git(repo_root, ["status", "--porcelain"]).await.map_err(|e| {
            SandboxError::NotARepo(format!("{}: {e}", repo_root.display()))
        })?;
        let dirty: Vec<&str> = status
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter(|l| {
                let index = l.chars().next().unwrap_or(' ');
                let worktree = l.chars().nth(1).unwrap_or(' ');
                index != '?' && (index != ' ' || worktree != ' ')
            })
            .collect();
        if !dirty.is_empty() {
            return Err(SandboxError::DirtyRepo(dirty.join("\n")));
        }
        Ok(())
    }

    /// Creates a linked worktree at `<repo-parent>/.jj-worktrees/<repo>-<task>`
    /// on new branch `jj/<task-id>` from `HEAD`.
    pub async fn create(repo_root: &Path, task_id: &str) -> Result<Self, SandboxError> {
        Self::require_clean_repo(repo_root).await?;
        let repo_root = repo_root.canonicalize()?;
        let repo_name = repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());
        let parent = repo_root
            .parent()
            .ok_or_else(|| SandboxError::NotARepo("repo root has no parent".into()))?;
        let worktree_dir = parent
            .join(".jj-worktrees")
            .join(format!("{repo_name}-{task_id}"));
        if worktree_dir.exists() {
            return Err(SandboxError::Git(
                "worktree create",
                format!(
                    "worktree already exists: {} (previous run not disposed?)",
                    worktree_dir.display()
                ),
            ));
        }
        let branch = format!("jj/{task_id}");
        std::fs::create_dir_all(worktree_dir.parent().unwrap())?;
        git(
            &repo_root,
            [
                "worktree",
                "add",
                "--quiet",
                "-b",
                &branch,
                worktree_dir.to_string_lossy().as_ref(),
                "HEAD",
            ],
        )
        .await
        .map_err(|e| SandboxError::Git("worktree add", e))?;

        Ok(Self {
            repo_root,
            worktree: worktree_dir,
            branch,
        })
    }

    /// Re-attaches to an existing worktree (resume path).
    pub fn attach(repo_root: &Path, task_id: &str) -> Result<Self, SandboxError> {
        let repo_root = repo_root.canonicalize()?;
        let repo_name = repo_root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string());
        let parent = repo_root
            .parent()
            .ok_or_else(|| SandboxError::NotARepo("repo root has no parent".to_string()))?;
        let worktree = parent
            .join(".jj-worktrees")
            .join(format!("{repo_name}-{task_id}"));
        if !worktree.exists() {
            return Err(SandboxError::Git(
                "worktree attach",
                format!("worktree not found: {}", worktree.display()),
            ));
        }
        Ok(Self {
            repo_root,
            worktree,
            branch: format!("jj/{task_id}"),
        })
    }

    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Commits every change in the worktree as a passing-step checkpoint.
    /// Returns the new commit sha.
    pub async fn checkpoint(&self, message: &str) -> Result<String, SandboxError> {
        let _ = git(&self.worktree, ["add", "-A"]).await;
        let status = git(&self.worktree, ["status", "--porcelain"])
            .await
            .map_err(|e| SandboxError::Git("status", e))?;
        if status.trim().is_empty() {
            return self.current_commit().await;
        }
        git(
            &self.worktree,
            ["commit", "--quiet", "-m", message, "--allow-empty"],
        )
        .await
        .map_err(|e| SandboxError::Git("checkpoint commit", e))?;
        self.current_commit().await
    }

    /// Reverts tracked modifications AND removes untracked files, restoring
    /// the worktree to the last checkpoint.
    pub async fn rollback(&self) -> Result<(), SandboxError> {
        let _ = git(&self.worktree, ["reset", "--hard", "--quiet", "HEAD"]).await;
        let _ = git(&self.worktree, ["clean", "-fd", "--quiet"]).await;
        Ok(())
    }

    pub async fn current_commit(&self) -> Result<String, SandboxError> {
        git(&self.worktree, ["rev-parse", "HEAD"])
            .await
            .map(|s| s.trim().to_string())
            .map_err(|e| SandboxError::Git("rev-parse", e))
    }

    /// Unified diff of all uncommitted changes (tracked + untracked via
    /// intent-to-add), used for Jev state and escalation payloads.
    pub async fn diff(&self) -> Result<String, SandboxError> {
        git(&self.worktree, ["add", "--intent-to-add", "-A"])
            .await
            .map_err(|e| SandboxError::Git("intent-to-add", e))?;
        let d = git(&self.worktree, ["diff", "HEAD"])
            .await
            .map_err(|e| SandboxError::Git("diff", e))?;
        Ok(d)
    }

    /// Atomically applies a unified diff from the frontier. Verifies first
    /// (`git apply --check`), then applies.
    pub async fn apply_patch(&self, patch: &str) -> Result<(), SandboxError> {
        let tmp_path = std::env::temp_dir().join(format!(
            "jj-patch-{}-{}.diff",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        use std::io::Write;
        let mut f = std::fs::File::create(&tmp_path)?;
        f.write_all(patch.as_bytes())?;
        f.sync_all().ok();
        let path = tmp_path.display().to_string();
        let result = async {
            git(&self.worktree, ["apply", "--check", &path])
                .await
                .map_err(|e| SandboxError::Git("apply --check", e))?;
            git(&self.worktree, ["apply", &path])
                .await
                .map_err(|e| SandboxError::Git("apply", e))?;
            Ok(())
        }
        .await;
        let _ = std::fs::remove_file(&tmp_path);
        result
    }

    /// True when the worktree has no uncommitted changes.
    pub async fn is_clean(&self) -> bool {
        matches!(git(&self.worktree, ["status", "--porcelain"]).await, Ok(s) if s.trim().is_empty())
    }

    /// Removes the worktree directory but KEEPS the branch (review/merge
    /// happens on `jj/<task-id>` after the run).
    pub async fn dispose(&self) -> Result<(), SandboxError> {
        git(&self.repo_root, ["worktree", "remove", "--force", &self.worktree.to_string_lossy()])
            .await
            .map_err(|e| SandboxError::Git("worktree remove", e))?;
        let _ = git(&self.repo_root, ["worktree", "prune"]).await;
        Ok(())
    }
}

async fn git<I, S>(cwd: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd).args(
        args.into_iter()
            .map(|a| a.as_ref().to_string()),
    );
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let output = cmd.output().await.map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        git(&root, ["init", "--quiet"]).await.unwrap();
        git(&root, ["config", "user.email", "jj@test"]).await.unwrap();
        git(&root, ["config", "user.name", "jj"]).await.unwrap();
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        git(&root, ["add", "-A"]).await.unwrap();
        git(&root, ["commit", "--quiet", "-m", "init"]).await.unwrap();
        (tmp, root)
    }

    #[tokio::test]
    async fn worktree_isolation_and_checkpoint_rollback() {
        let (_tmp, root) = init_repo().await;
        let sb = GitSandbox::create(&root, "t1").await.unwrap();
        assert!(sb.worktree().exists());
        assert_ne!(sb.worktree(), root);

        std::fs::create_dir_all(sb.worktree().join("src")).unwrap();
        std::fs::write(sb.worktree().join("src/new.rs"), "pub fn a() {}\n").unwrap();
        std::fs::write(sb.worktree().join("README.md"), "modified\n").unwrap();
        assert!(!sb.is_clean().await);

        let cp = sb.checkpoint("jj: step one verified").await.unwrap();
        assert!(sb.is_clean().await);
        assert_eq!(sb.current_commit().await.unwrap(), cp);

        std::fs::write(sb.worktree().join("README.md"), "broken\n").unwrap();
        std::fs::write(sb.worktree().join("junk.txt"), "junk\n").unwrap();
        sb.rollback().await.unwrap();
        assert!(sb.is_clean().await);
        assert!(!sb.worktree().join("junk.txt").exists(), "untracked files must be cleaned");
        assert_eq!(
            std::fs::read_to_string(sb.worktree().join("README.md")).unwrap(),
            "modified\n",
            "rollback restores last checkpoint, not the original commit"
        );

        sb.dispose().await.unwrap();
        assert!(!sb.worktree().exists());
        let branches = git(&root, ["branch", "--list", "jj/t1"]).await.unwrap();
        assert!(branches.contains("jj/t1"), "branch survives dispose for review/merge");
        let _ = git(&root, ["branch", "-D", "jj/t1"]).await;
    }

    #[tokio::test]
    async fn dirty_repo_is_rejected() {
        let (_tmp, root) = init_repo().await;
        std::fs::write(root.join("README.md"), "dirty\n").unwrap();
        assert!(matches!(
            GitSandbox::require_clean_repo(&root).await,
            Err(SandboxError::DirtyRepo(_))
        ));
    }

    #[tokio::test]
    async fn untracked_files_in_main_checkout_are_tolerated() {
        let (_tmp, root) = init_repo().await;
        std::fs::write(root.join("scratch.txt"), "untracked\n").unwrap();
        assert!(GitSandbox::require_clean_repo(&root).await.is_ok());
    }

    #[tokio::test]
    async fn apply_patch_checks_first() {
        let (_tmp, root) = init_repo().await;
        let sb = GitSandbox::create(&root, "t2").await.unwrap();
        let good_patch = "--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-hello\n+patched\n";
        sb.apply_patch(good_patch).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(sb.worktree().join("README.md")).unwrap(),
            "patched\n"
        );

        let bad_patch = "--- a/missing.rs\n+++ b/missing.rs\n@@ -1 +1 @@\n-x\n+y\n";
        assert!(sb.apply_patch(bad_patch).await.is_err());
        sb.dispose().await.unwrap();
        let _ = git(&root, ["branch", "-D", "jj/t2"]).await;
    }

    #[tokio::test]
    async fn diff_captures_changes() {
        let (_tmp, root) = init_repo().await;
        let sb = GitSandbox::create(&root, "t3").await.unwrap();
        std::fs::write(sb.worktree().join("README.md"), "changed\n").unwrap();
        let d = sb.diff().await.unwrap();
        assert!(d.contains("+changed"));
        sb.dispose().await.unwrap();
        let _ = git(&root, ["branch", "-D", "jj/t3"]).await;
    }
}
