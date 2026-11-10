//! Past sessions of the agents, whether or not Nyx Agents started them: their
//! record files, a plain-text look at the transcript, moving them to the Trash,
//! and resuming them in a terminal.

use std::path::{Path, PathBuf};

/// The agents whose past sessions are listed.
pub const AGENTS: &[&str] = crate::usage::AGENTS;

fn find(dir: &Path, depth: usize, matches: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if matches(&p) {
            out.push(p);
        } else if depth > 0 && e.file_type().is_ok_and(|t| t.is_dir()) {
            find(&p, depth - 1, matches, out);
        }
    }
}

fn plausible_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The files that hold a session: Claude Code's transcript and its subagents'
/// folder; Codex's rollouts (one per resume).
pub fn records(harness: &str, id: &str) -> Vec<PathBuf> {
    if !plausible_id(id) {
        return vec![];
    }
    let mut out = vec![];
    match harness {
        "claude" => {
            let projects = crate::usage::claude::base().join("projects");
            let name = format!("{id}.jsonl");
            find(&projects, 1, &|p| p.file_name().is_some_and(|n| n == name.as_str() || n == id), &mut out);
        }
        "codex" => {
            let base = crate::usage::codex::base();
            let is_rollout = |p: &Path| {
                p.extension().is_some_and(|x| x == "jsonl") && p.file_name().is_some_and(|n| n.to_string_lossy().contains(id))
            };
            for d in ["sessions", "archived_sessions"] {
                find(&base.join(d), 4, &is_rollout, &mut out);
            }
        }
        _ => {}
    }
    out.sort();
    out
}

/// The transcript as plain text, at most `max` bytes from its end.
pub fn peek(harness: &str, id: &str, max: usize) -> Option<String> {
    let files: Vec<PathBuf> = records(harness, id).into_iter().filter(|p| p.is_file()).collect();
    let text = match harness {
        "claude" => crate::claude::render_file(files.first()?),
        "codex" => {
            let mut all = vec![];
            for f in &files {
                let body = std::fs::read_to_string(f).ok()?;
                all.extend(body.lines().filter_map(|l| serde_json::from_str(l).ok()));
            }
            crate::codex::render(&all)
        }
        _ => return None,
    };
    if text.len() <= max {
        return Some(text);
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(format!("…{}", &text[start..]))
}

/// Move files to the user's Trash (the freedesktop layout, so they can be
/// restored from the file manager). Files must be on the home folder's disk.
pub fn trash(paths: &[PathBuf]) -> std::io::Result<()> {
    let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".local/share")).join("Trash");
    let (files, info) = (base.join("files"), base.join("info"));
    std::fs::create_dir_all(&files)?;
    std::fs::create_dir_all(&info)?;
    for p in paths {
        let stem = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "session".into());
        let mut name = stem.clone();
        let mut n = 1;
        while files.join(&name).exists() || info.join(format!("{name}.trashinfo")).exists() {
            n += 1;
            name = format!("{stem}.{n}");
        }
        let now = unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
        };
        let abs = std::fs::canonicalize(p)?;
        let path: String = abs
            .to_string_lossy()
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        std::fs::write(info.join(format!("{name}.trashinfo")), format!("[Trash Info]\nPath={path}\nDeletionDate={now}\n"))?;
        if let Err(e) = std::fs::rename(&abs, files.join(&name)) {
            let _ = std::fs::remove_file(info.join(format!("{name}.trashinfo")));
            return Err(e);
        }
    }
    Ok(())
}

/// The command that runs `argv` in the user's terminal in `cwd`:
/// `[sessions] terminal = ["foot", "-e"]` in config.toml, else xdg-terminal-exec.
pub fn terminal_command(argv: &[String], cwd: &str) -> Vec<String> {
    let cfg: toml::Table = std::fs::read_to_string(crate::paths::config_dir().join("config.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    let custom: Option<Vec<String>> = cfg
        .get("sessions")
        .and_then(|s| s.get("terminal"))
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .filter(|v: &Vec<String>| !v.is_empty());
    let mut cmd = custom.unwrap_or_else(|| vec!["xdg-terminal-exec".into(), format!("--dir={cwd}"), "--".into()]);
    cmd.extend(argv.iter().cloned());
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_odd_ids() {
        assert!(records("claude", "../../etc").is_empty());
        assert!(records("claude", "").is_empty());
    }

    #[test]
    fn trash_keeps_a_restorable_record() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_DATA_HOME", dir.path().join("data")) };
        let f = dir.path().join("a b.jsonl");
        std::fs::write(&f, "x").unwrap();
        trash(std::slice::from_ref(&f)).unwrap();
        assert!(!f.exists());
        let t = dir.path().join("data/Trash");
        assert_eq!(std::fs::read_to_string(t.join("files/a b.jsonl")).unwrap(), "x");
        let info = std::fs::read_to_string(t.join("info/a b.jsonl.trashinfo")).unwrap();
        assert!(info.contains("a%20b.jsonl") && info.starts_with("[Trash Info]\nPath=/"), "{info}");
        unsafe { std::env::remove_var("XDG_DATA_HOME") };
    }

    #[test]
    fn terminal_runs_the_argv_after_its_own() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let argv = vec!["codex".to_string(), "resume".to_string(), "x".to_string()];
        assert_eq!(terminal_command(&argv, "/p"), ["xdg-terminal-exec", "--dir=/p", "--", "codex", "resume", "x"]);
        std::fs::create_dir_all(dir.path().join("nyx-agents")).unwrap();
        std::fs::write(dir.path().join("nyx-agents/config.toml"), "[sessions]\nterminal = [\"foot\", \"-e\"]\n").unwrap();
        assert_eq!(terminal_command(&argv, "/p"), ["foot", "-e", "codex", "resume", "x"]);
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }
}
