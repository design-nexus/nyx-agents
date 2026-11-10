//! Git worktrees for agent sessions, so agents can work in parallel without
//! sharing a checkout. Created under `~/.local/share/nyx-agents/worktrees/
//! <repo>/<name>` on a new branch `agents/<name>`; removed only when clean,
//! and the branch is always kept.

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".local/share"))
        .join("nyx-agents/worktrees")
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(crate::sanitize::line(&String::from_utf8_lossy(&out.stderr), 300))
    }
}

/// A short name safe as a folder and a branch component.
pub fn slug(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').chars().take(40).collect()
}

/// Create a worktree of the repository containing `dir`, on a new branch.
/// Returns its folder.
pub fn create(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let (_, repo) = crate::git::context(dir).ok_or("not in a git repository")?;
    let name = slug(name);
    if name.is_empty() {
        return Err("the worktree needs a name".into());
    }
    let branch = format!("agents/{name}");
    git(&repo, &["check-ref-format", "--branch", &branch]).map_err(|_| format!("{branch} is not a usable branch name"))?;
    let repo_name = slug(&repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into()));
    let path = root().join(repo_name).join(&name);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let p = path.to_string_lossy().into_owned();
    git(&repo, &["worktree", "add", "-b", &branch, "--", &p, "HEAD"])?;
    Ok(path)
}

/// Remove a worktree made here, if it has no uncommitted changes. The branch stays.
pub fn remove(path: &Path) -> Result<(), String> {
    let path = path.canonicalize().map_err(|_| "the worktree is already gone".to_string())?;
    let ours = root().canonicalize().map(|r| path.starts_with(r)).unwrap_or(false);
    if !ours {
        return Err("only worktrees Nyx Agents created can be removed here".into());
    }
    if !git(&path, &["status", "--porcelain"])?.is_empty() {
        return Err("the worktree has uncommitted changes; commit or discard them first".into());
    }
    let p = path.to_string_lossy().into_owned();
    git(&path, &["worktree", "remove", "--", &p])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_removes_only_clean_worktrees() {
        let _env = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let t = tempfile::tempdir().unwrap();
        // SAFETY: no other test in this module reads XDG_DATA_HOME.
        unsafe { std::env::set_var("XDG_DATA_HOME", t.path().join("data")) };
        let repo = t.path().join("My Repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| assert!(Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap().status.success());
        run(&["init", "-q"]);
        run(&["-c", "user.email=a@b", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "init"]);
        let wt = create(&repo, "Fix the parser!").unwrap();
        assert!(wt.ends_with("my-repo/fix-the-parser"), "{}", wt.display());
        assert_eq!(git(&wt, &["branch", "--show-current"]).unwrap(), "agents/fix-the-parser");
        std::fs::write(wt.join("x"), "dirty").unwrap();
        assert!(remove(&wt).unwrap_err().contains("uncommitted"));
        std::fs::remove_file(wt.join("x")).unwrap();
        remove(&wt).unwrap();
        assert!(!wt.exists());
        assert!(git(&repo, &["rev-parse", "--verify", "agents/fix-the-parser"]).is_ok(), "branch kept");
        assert!(remove(&repo).is_err(), "never a folder it didn't make");
    }
}
