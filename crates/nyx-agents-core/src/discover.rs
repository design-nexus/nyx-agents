//! Finding harness executables without running anything that could install.
//!
//! mise shims and command wrappers are symlinks to the `mise` binary, and
//! running one can download and install a tool. They are skipped; the real
//! executables under mise's install directory are found instead.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn is_executable(p: &Path) -> bool {
    p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// True when `p` is (or links to) mise itself.
pub fn is_mise_trampoline(p: &Path) -> bool {
    let target = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    target.file_name().is_some_and(|n| n == "mise") || p.components().any(|c| c.as_os_str() == "shims")
}

/// The first real executable named one of `names`: on PATH (skipping mise
/// trampolines), then inside mise's installs for `mise_tools`.
pub fn find(names: &[&str], mise_tools: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for n in names {
            let p = dir.join(n);
            if is_executable(&p) && !is_mise_trampoline(&p) {
                return Some(p);
            }
        }
    }
    let installs = crate::paths::home().join(".local/share/mise/installs");
    for tool in mise_tools {
        let root = installs.join(tool).join("latest");
        if let Some(p) = search(&root, names, 4) {
            return Some(p);
        }
    }
    None
}

fn search(dir: &Path, names: &[&str], depth: usize) -> Option<PathBuf> {
    for n in names {
        let p = dir.join(n);
        if is_executable(&p) && !is_mise_trampoline(&p) {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subdirs.sort();
    subdirs.into_iter().find_map(|d| search(&d, names, depth - 1))
}

/// `<exe> --version`, first line, with a time limit. Only call on paths
/// returned by [`find`].
pub fn version(exe: &Path) -> Option<String> {
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("NO_COLOR", "1")
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(crate::sanitize::line(line, 80))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_mise_links() {
        let t = tempfile::tempdir().unwrap();
        let mise = t.path().join("mise");
        std::fs::write(&mise, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&mise, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = t.path().join("claude");
        std::os::unix::fs::symlink(&mise, &link).unwrap();
        assert!(is_mise_trampoline(&link));
        assert!(!is_mise_trampoline(Path::new("/bin/sh")));
    }
}
