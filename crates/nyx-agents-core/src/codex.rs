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

/// The rollout file of session `id` (live rollouts are plain JSONL).
pub fn rollout(id: &str, since_ms: u64) -> Option<PathBuf> {
    day_dirs(since_ms).into_iter().find_map(|d| {
        std::fs::read_dir(d).ok()?.flatten().map(|e| e.path()).find(|p| {
            p.extension().is_some_and(|x| x == "jsonl") && p.file_name().is_some_and(|n| n.to_string_lossy().contains(id))
        })
    })
}

/// Follows the subagents one Codex session starts: their rollouts name the
/// parent in `session_meta.source.subagent.thread_spawn.parent_thread_id`.
#[derive(Default)]
pub struct Subagents {
    known: std::collections::HashMap<PathBuf, (nyx_agents_proto::Subagent, crate::claude::Tail)>,
    checked: std::collections::HashSet<PathBuf>,
}

impl Subagents {
    pub fn refresh(&mut self, parent: &str, since_ms: u64) -> Option<Vec<nyx_agents_proto::Subagent>> {
        use nyx_agents_proto::{Subagent, SubagentSource, SubagentState};
        let mut changed = false;
        for dir in day_dirs(since_ms) {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "jsonl") || self.known.contains_key(&p) || !self.checked.insert(p.clone()) {
                    continue;
                }
                let Some(v) = first_line(&p) else {
                    // Not written yet; look again next time.
                    self.checked.remove(&p);
                    continue;
                };
                let spawn = v.pointer("/payload/source/subagent/thread_spawn");
                if spawn.and_then(|s| s.get("parent_thread_id")).and_then(Value::as_str) != Some(parent) {
                    continue;
                }
                let spawn = spawn.unwrap();
                let text = |k: &str| spawn.get(k).and_then(Value::as_str).map(|s| crate::sanitize::line(s, 80));
                let id: String = v
                    .pointer("/payload/id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                if self.known.values().any(|(k, _)| k.id == id) {
                    continue;
                }
                let sub = Subagent {
                    id,
                    name: text("agent_role").filter(|r| !r.is_empty()).unwrap_or_else(|| "Agent".into()),
                    description: text("agent_nickname"),
                    source: SubagentSource::Transcript,
                    state: SubagentState::Running,
                    started: modified_ms(&p),
                    ended: None,
                };
                self.known.insert(p, (sub, crate::claude::Tail::default()));
                changed = true;
            }
        }
        for (path, (sub, tail)) in self.known.iter_mut() {
            for r in tail.read(path) {
                let state = match r.pointer("/payload/type").and_then(Value::as_str) {
                    Some("task_complete") => Some(SubagentState::Completed),
                    Some("turn_aborted") => Some(SubagentState::Failed),
                    Some("task_started") => Some(SubagentState::Running),
                    _ => None,
                };
                if let Some(state) = state
                    && state != sub.state
                {
                    sub.state = state;
                    sub.ended = (state != SubagentState::Running).then(nyx_agents_proto::now_ms);
                    changed = true;
                }
            }
        }
        changed.then(|| {
            let mut list: Vec<_> = self.known.values().map(|(s, _)| s.clone()).collect();
            list.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
            list
        })
    }
}

fn plain(s: &str) -> String {
    s.chars().filter(|c| *c == '\n' || !c.is_control()).collect()
}

fn texts(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            items.iter().filter_map(|i| i.get("text").and_then(Value::as_str).or(i.as_str())).collect::<Vec<_>>().join("\n")
        }
        _ => String::new(),
    }
}

fn first_lines(s: &str, n: usize, width: usize) -> Vec<String> {
    let all: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut out: Vec<String> = all.iter().take(n).map(|l| l.chars().take(width).collect()).collect();
    if all.len() > n {
        out.push(format!("… +{} lines", all.len() - n));
    }
    out
}

/// Render rollout records as terminal text: the task, messages, tool calls
/// with their main argument, short results, and how the task ended.
pub fn render(records: &[Value]) -> String {
    let mut out = String::new();
    let mut push = |l: &str| {
        out.push_str(l);
        out.push_str("\r\n");
    };
    for r in records {
        let p = &r["payload"];
        match (r["type"].as_str(), p["type"].as_str()) {
            (Some("response_item"), Some("message")) => {
                let text = plain(&texts(&p["content"]));
                match p["role"].as_str() {
                    Some("user") => {
                        push("\x1b[2m▸ Task\x1b[0m");
                        for l in first_lines(&text, 8, 160) {
                            push(&format!("\x1b[2m  {l}\x1b[0m"));
                        }
                        push("");
                    }
                    Some("assistant") => {
                        for l in first_lines(&text, 40, 200) {
                            push(&l);
                        }
                        push("");
                    }
                    _ => {}
                }
            }
            (Some("response_item"), Some("reasoning")) => push("\x1b[2m✻ Thinking…\x1b[0m"),
            (Some("response_item"), Some(kind @ ("function_call" | "custom_tool_call"))) => {
                let name = plain(p["name"].as_str().unwrap_or("tool"));
                let raw = if kind == "function_call" {
                    p["arguments"].as_str().unwrap_or("")
                } else {
                    p["input"].as_str().unwrap_or("")
                };
                let parsed: Option<Value> = serde_json::from_str(raw).ok();
                let arg = parsed
                    .as_ref()
                    .and_then(|a| {
                        ["cmd", "command", "path", "file_path", "pattern", "query", "message"]
                            .iter()
                            .find_map(|k| a.get(*k))
                            .cloned()
                    })
                    .map(|v| texts(&v))
                    .unwrap_or_else(|| raw.to_string());
                let arg: String = plain(&arg).lines().next().unwrap_or("").chars().take(100).collect();
                push(&format!("\x1b[36m●\x1b[0m \x1b[1m{name}({arg})\x1b[0m"));
            }
            (Some("response_item"), Some("function_call_output" | "custom_tool_call_output")) => {
                let lines = first_lines(&plain(&texts(&p["output"])), 3, 140);
                match lines.split_first() {
                    None => push("\x1b[2m  ⎿ (no output)\x1b[0m"),
                    Some((first, rest)) => {
                        push(&format!("\x1b[2m  ⎿ {first}\x1b[0m"));
                        for l in rest {
                            push(&format!("\x1b[2m    {l}\x1b[0m"));
                        }
                    }
                }
            }
            (Some("event_msg"), Some("task_complete")) => push("\x1b[32m✓ Done\x1b[0m"),
            (Some("event_msg"), Some("turn_aborted")) => {
                push(&format!("\x1b[31m✗ Stopped: {}\x1b[0m", plain(p["reason"].as_str().unwrap_or(""))))
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_sessions_and_follows_subagents() {
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

        // Subagents of a session (same test: both use CODEX_HOME).
        let child = dir.join("rollout-2026-11-10T00-00-01-kid1.jsonl");
        std::fs::write(&child, concat!(
            r#"{"type":"session_meta","payload":{"id":"kid1","cwd":"/w","source":{"subagent":{"thread_spawn":{"parent_thread_id":"dad","agent_nickname":"Ada","agent_role":"explorer","depth":1}}}}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Find the parser"}]}}"#, "\n",
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"rg -n vt_write \u001b]52;c;aGk=\u0007"}}"#, "\n",
        )).unwrap();
        let mut s = Subagents::default();
        let list = s.refresh("dad", since).unwrap();
        assert_eq!(list.len(), 1, "{:?}", s.known.keys().collect::<Vec<_>>());
        assert_eq!(
            (list[0].name.as_str(), list[0].description.as_deref(), list[0].state),
            ("explorer", Some("Ada"), nyx_agents_proto::SubagentState::Running)
        );
        use std::io::Write;
        writeln!(
            std::fs::OpenOptions::new().append(true).open(&child).unwrap(),
            r#"{{"type":"event_msg","payload":{{"type":"task_complete"}}}}"#
        )
        .unwrap();
        assert_eq!(s.refresh("dad", since).unwrap()[0].state, nyx_agents_proto::SubagentState::Completed);
        assert!(Subagents::default().refresh("someone-else", since).is_none());
        let recs: Vec<Value> =
            std::fs::read_to_string(&child).unwrap().lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        let out = render(&recs);
        assert!(out.contains("exec(rg -n vt_write") && !out.contains("\u{1b}]52"), "{out:?}");
        assert!(out.contains("✓ Done"));
    }
}
