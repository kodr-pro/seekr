use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SandboxError {
    #[error("path escapes the worktree sandbox: {0}")]
    NotUnderRoot(String),
    #[error("path is outside the step's allowed_paths: {0}")]
    NotAllowed(String),
    #[error("untrusted path rejected: {0}")]
    Invalid(String),
    #[error("sandbox I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Fail-closed path containment for unattended worker file operations.
///
/// Reads are permitted anywhere inside the worktree root. Writes are
/// permitted only beneath one of the step's canonical `allowed_paths`
/// prefixes; an empty allowlist means the step is read-only. Absolute
/// paths, `..` traversal, and symlink escapes are rejected.
#[derive(Clone, Debug)]
pub struct PathSandbox {
    root: PathBuf,
    allowed: Vec<PathBuf>,
}

impl PathSandbox {
    pub fn new(
        root: &Path,
        allowed_paths: &[PathBuf],
    ) -> Result<Self, SandboxError> {
        let root = root.canonicalize().map_err(|e| {
            SandboxError::Io(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", root.display()),
            ))
        })?;
        if !allowed_paths.iter().all(|p| !p.as_os_str().is_empty()) {
            return Err(SandboxError::Invalid("empty allowlist entry".into()));
        }
        let allowed = allowed_paths
            .iter()
            .map(|p| Self::anchor(&root, p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { root, allowed })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_read_only(&self) -> bool {
        self.allowed.is_empty()
    }

    /// Canonical allowed prefixes for display in worker prompts.
    pub fn allowed(&self) -> &[PathBuf] {
        &self.allowed
    }

    /// Resolves an untrusted path string for a read: must exist and be a
    /// real path inside the worktree.
    pub fn resolve_for_read(
        &self,
        untrusted: &str,
    ) -> Result<PathBuf, SandboxError> {
        let anchored = Self::anchor(&self.root, Path::new(untrusted.trim()))?;
        let canonical = anchored.canonicalize().map_err(|_| {
            SandboxError::Invalid(format!("path does not exist: {untrusted}"))
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(SandboxError::NotUnderRoot(untrusted.to_string()));
        }
        Ok(canonical)
    }

    /// Resolves an untrusted path string for a write: the target may not
    /// exist yet, but its deepest existing ancestor (canonicalized) must be
    /// inside the worktree, and the effective target must fall under an
    /// allowed prefix. Fails closed on any resolution doubt.
    pub fn resolve_for_write(
        &self,
        untrusted: &str,
    ) -> Result<PathBuf, SandboxError> {
        if self.is_read_only() {
            return Err(SandboxError::NotAllowed(format!(
                "step is read-only; write to '{untrusted}' denied"
            )));
        }
        let anchored = Self::anchor(&self.root, Path::new(untrusted.trim()))?;

        let (existing_ancestor, remainder) = split_existing(&anchored)?;
        let canonical_ancestor =
            existing_ancestor.canonicalize().map_err(|_| {
                SandboxError::Invalid(format!(
                    "cannot resolve parent of: {untrusted}"
                ))
            })?;
        if !canonical_ancestor.starts_with(&self.root) {
            return Err(SandboxError::NotUnderRoot(untrusted.to_string()));
        }
        let effective = canonical_ancestor.join(&remainder);
        let normalized = normalize_no_escape(&effective, &self.root)?;

        if anchored.exists()
            && let Ok(canonical_target) = anchored.canonicalize()
            && !canonical_target.starts_with(&self.root)
        {
            return Err(SandboxError::NotUnderRoot(untrusted.to_string()));
        }

        if !self
            .allowed
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
        {
            return Err(SandboxError::NotAllowed(untrusted.to_string()));
        }
        Ok(normalized)
    }

    fn anchor(root: &Path, untrusted: &Path) -> Result<PathBuf, SandboxError> {
        let joined = if untrusted.is_absolute() {
            let normalized = normalize_absolute(untrusted)?;
            if !normalized.starts_with(root) {
                return Err(SandboxError::NotUnderRoot(
                    untrusted.display().to_string(),
                ));
            }
            normalized
        } else {
            normalize_no_escape(&root.join(untrusted), root)?
        };
        Ok(joined)
    }
}

/// Lexically normalizes `..`/`.` without touching the filesystem and
/// rejects any climb above `root`.
fn normalize_no_escape(
    path: &Path,
    root: &Path,
) -> Result<PathBuf, SandboxError> {
    let mut stack: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                _ => {
                    return Err(SandboxError::NotUnderRoot(
                        path.display().to_string(),
                    ));
                }
            },
            other => stack.push(other),
        }
    }
    let normalized: PathBuf = stack.iter().collect();
    if normalized.starts_with(root) {
        Ok(normalized)
    } else {
        Err(SandboxError::NotUnderRoot(path.display().to_string()))
    }
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, SandboxError> {
    let mut stack: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                _ => {
                    return Err(SandboxError::NotUnderRoot(
                        path.display().to_string(),
                    ));
                }
            },
            other => stack.push(other),
        }
    }
    Ok(stack.iter().collect())
}

/// Splits a path into its deepest existing ancestor and the remainder.
fn split_existing(path: &Path) -> Result<(PathBuf, PathBuf), SandboxError> {
    let mut current = path.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if current.exists() || current.parent().is_none() {
            let rem: PathBuf = remainder.iter().rev().collect();
            return Ok((current, rem));
        }
        match current.file_name() {
            Some(name) => remainder.push(name.to_os_string()),
            None => {
                return Err(SandboxError::Invalid(path.display().to_string()));
            }
        }
        if !current.pop() {
            let rem: PathBuf = remainder.iter().rev().collect();
            return Ok((current, rem));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox_with(allowed: &[&str]) -> (tempfile::TempDir, PathSandbox) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn f() {}").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\n").unwrap();
        let allowed: Vec<PathBuf> = allowed.iter().map(PathBuf::from).collect();
        let sb = PathSandbox::new(&root, &allowed).unwrap();
        (tmp, sb)
    }

    #[test]
    fn write_allowed_under_prefix() {
        let (_tmp, sb) = sandbox_with(&["src"]);
        let p = sb.resolve_for_write("src/new_file.rs").unwrap();
        assert!(p.starts_with(sb.root().join("src")));
        let p2 = sb.resolve_for_write("src/deep/nested/file.rs").unwrap();
        assert!(p2.starts_with(sb.root().join("src")));
    }

    #[test]
    fn write_denied_outside_allowed_prefix() {
        let (_tmp, sb) = sandbox_with(&["src"]);
        assert!(matches!(
            sb.resolve_for_write("Cargo.toml"),
            Err(SandboxError::NotAllowed(_))
        ));
        assert!(matches!(
            sb.resolve_for_write("tests/foo.rs"),
            Err(SandboxError::NotAllowed(_))
        ));
    }

    #[test]
    fn sibling_prefix_does_not_match() {
        let (_tmp, sb) = sandbox_with(&["src"]);
        assert!(matches!(
            sb.resolve_for_write("src-evil/pwned.rs"),
            Err(SandboxError::NotAllowed(_))
        ));
    }

    #[test]
    fn traversal_is_rejected() {
        let (_tmp, sb) = sandbox_with(&["src"]);
        assert!(matches!(
            sb.resolve_for_write("src/../../outside.rs"),
            Err(SandboxError::NotUnderRoot(_))
        ));
        assert!(matches!(
            sb.resolve_for_write("../../etc/passwd"),
            Err(SandboxError::NotUnderRoot(_))
        ));
    }

    #[test]
    fn absolute_paths_outside_root_rejected() {
        let (_tmp, sb) = sandbox_with(&["src"]);
        assert!(matches!(
            sb.resolve_for_read("/etc/passwd"),
            Err(SandboxError::NotUnderRoot(_))
        ));
        assert!(matches!(
            sb.resolve_for_write("/etc/passwd"),
            Err(SandboxError::NotUnderRoot(_))
        ));
    }

    #[test]
    fn symlink_escape_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let outside = tmp.path().parent().unwrap().join("jj-outside-target");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("src/link")).unwrap();

        let sb = PathSandbox::new(&root, &[PathBuf::from("src")]).unwrap();
        assert!(matches!(
            sb.resolve_for_write("src/link/pwned.rs"),
            Err(SandboxError::NotUnderRoot(_))
        ));
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn read_only_step_denies_all_writes() {
        let (_tmp, sb) = sandbox_with(&[]);
        assert!(sb.is_read_only());
        assert!(matches!(
            sb.resolve_for_write("src/lib.rs"),
            Err(SandboxError::NotAllowed(_))
        ));
    }

    #[test]
    fn reads_allowed_anywhere_in_root() {
        let (_tmp, sb) = sandbox_with(&["src"]);
        let p = sb.resolve_for_read("src/lib.rs").unwrap();
        assert!(p.ends_with("src/lib.rs"));
        assert!(sb.resolve_for_read("Cargo.toml").is_ok());
        assert!(matches!(
            sb.resolve_for_read("does/not/exist.rs"),
            Err(SandboxError::Invalid(_))
        ));
    }

    #[test]
    fn dot_working_directory_is_not_a_sandbox_bypass() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let sb = PathSandbox::new(&root, &[PathBuf::from("src")]).unwrap();
        assert!(sb.resolve_for_write("./src/ok.rs").is_ok());
        assert!(matches!(
            sb.resolve_for_write("src/../Cargo.toml"),
            Err(SandboxError::NotAllowed(_))
        ));
    }
}
