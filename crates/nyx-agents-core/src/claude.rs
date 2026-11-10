//! Claude Code's records, read only.
//!
//! Each subagent the parent starts with the `Agent` tool gets a transcript
//! and a meta file beside the parent's transcript:
//!
//! ```text
//! ~/.claude/projects/<cwd slug>/<session>.jsonl                  parent
//! ~/.claude/projects/<cwd slug>/<session>/subagents/agent-<id>.jsonl
//! ~/.claude/projects/<cwd slug>/<session>/subagents/agent-<id>.meta.json
//! ```
//!
//! A subagent is finished when the parent's transcript holds the result of
//! the `Agent` call (`toolUseId` in the meta file); `is_error` means it
//! failed. A background agent's call returns at once, so its own
//! transcript ending a turn is what marks it done.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use nyx_agents_proto::{Subagent, SubagentSource, SubagentState};
use serde_json::Value;

use crate::adapters::claude_transcript;

pub fn subagents_dir(cwd: &str, session: &str) -> PathBuf {
    claude_transcript(cwd, session).with_extension("").join("subagents")
}

pub fn subagent_transcript(cwd: &str, session: &str, agent: &str) -> PathBuf {
    subagents_dir(cwd, session).join(format!("agent-{agent}.jsonl"))
}

/// Reads a JSONL file from where it left off; a line still being written
/// waits for the next call.
#[derive(Default)]
pub struct Tail {
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    pub fn read(&mut self, path: &Path) -> Vec<Value> {
        let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // Rewritten from the start.
            self.offset = 0;
            self.partial.clear();
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut buf = Vec::new();
        // Bounded per call, so a huge file is taken in steps.
        let _ = (&mut f).take(8 << 20).read_to_end(&mut buf);
        self.offset += buf.len() as u64;
        let mut data = std::mem::take(&mut self.partial);
        data.extend_from_slice(&buf);
        let mut out = Vec::new();
        let mut rest: &[u8] = &data;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            if let Ok(v) = serde_json::from_slice::<Value>(&rest[..i]) {
                out.push(v);
            }
            rest = &rest[i + 1..];
        }
        self.partial = rest.to_vec();
        out
    }
}

#[derive(Clone, Debug, Default)]
struct Call {
    result: Option<bool>,
    background: bool,
}

/// Follows one Claude session's subagents.
#[derive(Default)]
pub struct Subagents {
    parent: Tail,
    calls: HashMap<String, Call>,
    known: HashMap<String, (Subagent, String, Tail, bool)>,
}

fn blocks(v: &Value) -> impl Iterator<Item = &Value> {
    v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten()
}

fn millis(path: &Path) -> u64 {
    path.metadata()
        .and_then(|m| m.created().or_else(|_| m.modified()))
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or_else(nyx_agents_proto::now_ms)
}

impl Subagents {
    /// Read what changed; returns the current list when anything did.
    pub fn refresh(&mut self, cwd: &str, session: &str) -> Option<Vec<Subagent>> {
        let mut changed = false;
        for rec in self.parent.read(&claude_transcript(cwd, session)) {
            for b in blocks(&rec) {
                match b.get("type").and_then(Value::as_str) {
                    Some("tool_use") if b.get("name").and_then(Value::as_str) == Some("Agent") => {
                        let id = b.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
                        let bg = b.pointer("/input/run_in_background").and_then(Value::as_bool).unwrap_or(false);
                        self.calls.entry(id).or_default().background = bg;
                    }
                    Some("tool_result") => {
                        if let Some(id) = b.get("tool_use_id").and_then(Value::as_str)
                            && let Some(c) = self.calls.get_mut(id)
                        {
                            c.result = Some(b.get("is_error").and_then(Value::as_bool).unwrap_or(false));
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        let dir = subagents_dir(cwd, session);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(agent) = name.strip_prefix("agent-").and_then(|n| n.strip_suffix(".meta.json")) else { continue };
                if self.known.contains_key(agent) {
                    continue;
                }
                let Ok(meta) =
                    std::fs::read(e.path()).map_err(drop).and_then(|b| serde_json::from_slice::<Value>(&b).map_err(drop))
                else {
                    continue;
                };
                let text = |k: &str| meta.get(k).and_then(Value::as_str).map(|s| crate::sanitize::line(s, 160));
                let sub = Subagent {
                    id: agent.chars().filter(|c| c.is_ascii_alphanumeric()).collect(),
                    name: text("agentType").unwrap_or_else(|| "Agent".into()),
                    description: text("description"),
                    source: SubagentSource::Transcript,
                    state: SubagentState::Running,
                    started: millis(&e.path()),
                    ended: None,
                };
                let tool = meta.get("toolUseId").and_then(Value::as_str).unwrap_or_default().to_string();
                self.known.insert(agent.to_string(), (sub, tool, Tail::default(), false));
                changed = true;
            }
        }

        for (agent, (sub, tool, tail, ended_turn)) in self.known.iter_mut() {
            for rec in tail.read(&dir.join(format!("agent-{agent}.jsonl"))) {
                if rec.get("type").and_then(Value::as_str) == Some("assistant") {
                    *ended_turn = rec.pointer("/message/stop_reason").and_then(Value::as_str) == Some("end_turn");
                }
            }
            let call = self.calls.get(tool.as_str()).cloned().unwrap_or_default();
            let state = match call.result {
                Some(true) => SubagentState::Failed,
                Some(false) if !call.background => SubagentState::Completed,
                _ if call.background && *ended_turn => SubagentState::Completed,
                _ => SubagentState::Running,
            };
            if state != sub.state {
                sub.state = state;
                sub.ended = (state != SubagentState::Running).then(nyx_agents_proto::now_ms);
                changed = true;
            }
        }

        changed.then(|| {
            let mut list: Vec<Subagent> = self.known.values().map(|(s, ..)| s.clone()).collect();
            list.sort_by(|a, b| (a.started, &a.id).cmp(&(b.started, &b.id)));
            list
        })
    }
}

/// Text safe to show inside a terminal: no escape sequences or control
/// characters from the transcript reach the viewer.
fn plain(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            '\n' => Some('\n'),
            '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

fn clip(s: &str, lines: usize, width: usize) -> Vec<String> {
    let mut out: Vec<String> =
        s.lines().filter(|l| !l.trim().is_empty()).take(lines).map(|l| l.chars().take(width).collect()).collect();
    let total = s.lines().filter(|l| !l.trim().is_empty()).count();
    if total > lines {
        out.push(format!("… +{} lines", total - lines));
    }
    out
}

fn tool_summary(name: &str, input: &Value) -> String {
    let pick = ["file_path", "path", "pattern", "command", "url", "query", "description", "prompt"];
    let arg = pick.iter().find_map(|k| input.get(*k).and_then(Value::as_str)).unwrap_or("");
    let arg: String = plain(arg).lines().next().unwrap_or("").chars().take(100).collect();
    if arg.is_empty() { name.to_string() } else { format!("{name}({arg})") }
}

/// Render transcript records as terminal text in the style of Claude Code's
/// own view: what the agent says, the tools it runs and short results.
pub fn render(records: &[Value]) -> String {
    let mut out = String::new();
    let push = |out: &mut String, line: &str| {
        out.push_str(line);
        out.push_str("\r\n");
    };
    for r in records {
        let kind = r.get("type").and_then(Value::as_str).unwrap_or("");
        let content = r.pointer("/message/content");
        match (kind, content) {
            ("user", Some(Value::String(s))) => {
                push(&mut out, "\x1b[2m▸ Task\x1b[0m");
                for l in clip(&plain(s), 8, 160) {
                    push(&mut out, &format!("\x1b[2m  {l}\x1b[0m"));
                }
                push(&mut out, "");
            }
            (_, Some(Value::Array(items))) => {
                for b in items {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let t = plain(b.get("text").and_then(Value::as_str).unwrap_or(""));
                            for l in clip(&t, 40, 200) {
                                push(&mut out, &l);
                            }
                            push(&mut out, "");
                        }
                        Some("thinking") => push(&mut out, "\x1b[2m✻ Thinking…\x1b[0m"),
                        Some("tool_use") => {
                            let name = plain(b.get("name").and_then(Value::as_str).unwrap_or("Tool"));
                            let summary = tool_summary(&name, b.get("input").unwrap_or(&Value::Null));
                            push(&mut out, &format!("\x1b[36m●\x1b[0m \x1b[1m{summary}\x1b[0m"));
                        }
                        Some("tool_result") => {
                            let err = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                            let text = match b.get("content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(parts)) => parts
                                    .iter()
                                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            let lines = clip(&plain(&text), 3, 140);
                            let color = if err { "\x1b[31m" } else { "\x1b[2m" };
                            match lines.first() {
                                None => push(&mut out, &format!("{color}  ⎿ (no output)\x1b[0m")),
                                Some(first) => {
                                    push(&mut out, &format!("{color}  ⎿ {first}\x1b[0m"));
                                    for l in &lines[1..] {
                                        push(&mut out, &format!("{color}    {l}\x1b[0m"));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Read a whole transcript (bounded) and render it.
pub fn render_file(path: &Path) -> String {
    let Ok(f) = std::fs::File::open(path) else { return String::new() };
    let records: Vec<Value> =
        BufReader::new(f.take(32 << 20)).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
    render(&records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_lines(p: &Path, lines: &[Value]) {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn follows_subagents_to_completion() {
        let home = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module don't run threads that read HOME.
        unsafe { std::env::set_var("HOME", home.path()) };
        let cwd = "/work/repo";
        let parent = claude_transcript(cwd, "s1");
        let dir = subagents_dir(cwd, "s1");
        std::fs::create_dir_all(&dir).unwrap();
        write_lines(
            &parent,
            &[json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Agent", "input": {"description": "Find the parser", "prompt": "…", "subagent_type": "Explore"}}
            ]}})],
        );
        std::fs::write(
            dir.join("agent-abc.meta.json"),
            json!({"agentType": "Explore", "description": "Find the \u{202E}parser", "toolUseId": "toolu_1"}).to_string(),
        )
        .unwrap();
        write_lines(&dir.join("agent-abc.jsonl"), &[json!({"type": "user", "message": {"content": "Find it"}})]);

        let mut s = Subagents::default();
        let list = s.refresh(cwd, "s1").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].state), ("Explore", SubagentState::Running));
        assert_eq!(list[0].description.as_deref(), Some("Find the parser"), "bidi controls removed");
        assert!(s.refresh(cwd, "s1").is_none(), "nothing changed");

        write_lines(
            &parent,
            &[
                json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": "done"}]}}),
            ],
        );
        let list = s.refresh(cwd, "s1").unwrap();
        assert_eq!(list[0].state, SubagentState::Completed);
    }

    #[test]
    fn render_strips_escapes_and_summarises_tools() {
        let recs = vec![
            json!({"type": "user", "message": {"content": "Look \u{1b}]52;c;aGk=\u{7} around"}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "thinking", "thinking": "hmm"},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "src/lib.rs"}}
            ]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": "a\nb\nc\nd\ne"}]}}),
        ];
        let out = render(&recs);
        assert!(!out.contains("\u{1b}]52"), "no OSC from the transcript reaches the viewer");
        assert!(out.contains("Read(src/lib.rs)"));
        assert!(out.contains("… +2 lines"));
    }
}
