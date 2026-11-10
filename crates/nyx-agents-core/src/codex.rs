//! Codex CLI's records, read only.
//!
//! Each session is `~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`
//! whose first line is `session_meta` with `payload.{id, cwd, timestamp,
//! source}`. Codex cannot be given a session id up front, so the manager
//! recognises the session it started: the first new CLI rollout in the same
//! folder, begun after the launch.

use std::path::{Path, PathBuf};

use serde_json::Value;

fn codex_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".codex"))
}

/// Day folders that may hold a session started at `since_ms` (that day and
/// the next, across midnight).
pub fn day_dirs(since_ms: u64) -> Vec<PathBuf> {
    let root = codex_home().join("sessions");
    let mut out = Vec::new();
    for offset in [0u64, 86_400_000] {
        let t = (since_ms + offset) as i64 / 1000;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        unsafe { libc::localtime_r(&t, &mut tm) };
        out.push(root.join(format!("{:04}/{:02}/{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday)));
    }
    out.dedup();
    out
}

fn first_line(path: &Path) -> Option<Value> {
    use std::io::{BufRead, BufReader, Read};
    let f = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(f.take(256 << 10)).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

fn modified_ms(p: &Path) -> u64 {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The id of the CLI session in `cwd` begun after `since_ms`, skipping ids
/// already claimed by other sessions.
pub fn find_session(cwd: &str, since_ms: u64, taken: &[String]) -> Option<String> {
    let mut candidates: Vec<(u64, String)> = Vec::new();
    for dir in day_dirs(since_ms) {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "jsonl") || modified_ms(&p) + 5_000 < since_ms {
                continue;
            }
            let Some(v) = first_line(&p) else { continue };
            if v.get("type").and_then(Value::as_str) != Some("session_meta") {
                continue;
            }
            let pl = &v["payload"];
            let id = pl["id"].as_str().unwrap_or_default();
            let from_cli = matches!(pl["source"].as_str(), Some("cli") | None) && pl["source"].get("subagent").is_none();
            if id.is_empty() || !from_cli || pl["cwd"].as_str() != Some(cwd) || taken.iter().any(|t| t == id) {
                continue;
            }
            candidates.push((modified_ms(&p), id.to_string()));
        }
    }
    candidates.sort();
    candidates.into_iter().next().map(|(_, id)| id)
}

/// Whether a rollout for `id` exists (there is something to resume).
pub fn has_session(id: &str, since_ms: u64) -> bool {
    day_dirs(since_ms).iter().any(|d| {
        std::fs::read_dir(d).map(|it| it.flatten().any(|e| e.file_name().to_string_lossy().contains(id))).unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_new_cli_session_in_the_folder() {
        let t = tempfile::tempdir().unwrap();
        // SAFETY: no other test in this module reads CODEX_HOME.
        unsafe { std::env::set_var("CODEX_HOME", t.path()) };
        let since = nyx_agents_proto::now_ms();
        let dir = &day_dirs(since)[0];
        std::fs::create_dir_all(dir).unwrap();
        let meta = |id: &str, cwd: &str, source: &str| {
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","cwd":"{cwd}","source":"{source}"}}}}"#) + "\n"
        };
        std::fs::write(dir.join("rollout-a.jsonl"), meta("other-folder", "/elsewhere", "cli")).unwrap();
        std::fs::write(dir.join("rollout-b.jsonl"), meta("from-ide", "/work", "vscode")).unwrap();
        std::fs::write(dir.join("rollout-2026-11-10T00-00-00-ours.jsonl"), meta("ours", "/work", "cli")).unwrap();
        assert_eq!(find_session("/work", since, &[]).as_deref(), Some("ours"));
        assert_eq!(find_session("/work", since, &["ours".into()]), None);
        assert!(has_session("ours", since));
    }
}
