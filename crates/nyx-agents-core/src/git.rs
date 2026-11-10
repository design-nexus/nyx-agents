//! Repository context for a working directory, read from `.git` directly
//! (no git process, no locks).

use std::path::{Path, PathBuf};

use nyx_agents_proto::Worktree;

/// The worktree containing `dir`, its branch, and the main repository root.
pub fn context(dir: &Path) -> Option<(Worktree, PathBuf)> {
    let mut d = dir.canonicalize().ok()?;
    loop {
        let dotgit = d.join(".git");
        if dotgit.is_dir() {
            let branch = head_branch(&dotgit);
            return Some((Worktree { root: d.display().to_string(), branch, linked: false }, d));
        }
        if dotgit.is_file() {
            // A linked worktree: ".git" holds "gitdir: <main>/.git/worktrees/<name>".
            let text = std::fs::read_to_string(&dotgit).ok()?;
            let gitdir = PathBuf::from(text.strip_prefix("gitdir:")?.trim());
            let gitdir = if gitdir.is_absolute() { gitdir } else { d.join(gitdir) };
            let branch = head_branch(&gitdir);
            let main = std::fs::read_to_string(gitdir.join("commondir"))
                .ok()
                .map(|c| {
                    let c = PathBuf::from(c.trim());
                    if c.is_absolute() { c } else { gitdir.join(c) }
                })
                .and_then(|c| c.canonicalize().ok())
                .and_then(|c| c.parent().map(Path::to_path_buf))
                .unwrap_or_else(|| d.clone());
            return Some((Worktree { root: d.display().to_string(), branch, linked: true }, main));
        }
        if !d.pop() {
            return None;
        }
    }
}

fn head_branch(gitdir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    match head.trim().strip_prefix("ref: refs/heads/") {
        Some(b) => Some(b.to_string()),
        None => Some(format!("detached {}", &head.trim()[..head.trim().len().min(8)])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_main_and_linked_worktrees() {
        let t = tempfile::tempdir().unwrap();
        let main = t.path().join("repo");
        std::fs::create_dir_all(main.join(".git/worktrees/feat")).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(main.join("src")).unwrap();
        let (wt, root) = context(&main.join("src")).unwrap();
        assert_eq!((wt.branch.as_deref(), wt.linked), (Some("main"), false));
        assert_eq!(root, main.canonicalize().unwrap());

        let linked = t.path().join("feat");
        std::fs::create_dir_all(&linked).unwrap();
        let gd = main.join(".git/worktrees/feat");
        std::fs::write(linked.join(".git"), format!("gitdir: {}\n", gd.display())).unwrap();
        std::fs::write(gd.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        std::fs::write(gd.join("commondir"), "../..\n").unwrap();
        let (wt, root) = context(&linked).unwrap();
        assert_eq!((wt.branch.as_deref(), wt.linked), (Some("feature/x"), true));
        assert_eq!(root, main.canonicalize().unwrap());
    }
}
