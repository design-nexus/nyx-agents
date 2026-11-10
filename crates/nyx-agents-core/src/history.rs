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

/// Agents whose sessions are files of their own, so they can go to the Trash.
/// OpenCode, Hermes and Crush keep every session as rows of one database.
pub fn trashable(harness: &str) -> bool {
    matches!(harness, "claude" | "codex" | "grok" | "cursor" | "muse" | "antigravity" | "omp")
}

/// The files and folders that hold a session: Claude Code's transcript and its
/// subagents' folder; Codex's rollouts (one per resume); Grok's and Muse's session
/// folders; Cursor's chat folder and agent transcript; Antigravity's brain folder
/// and conversation database; oh-my-pi's session file.
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
        "grok" => find(&crate::usage::grok::base().join("sessions"), 1, &|p| p.is_dir() && p.file_name().is_some_and(|n| n == id), &mut out),
        "muse" => find(&crate::usage::muse::base().join("sessions"), 3, &|p| p.is_dir() && p.file_name().is_some_and(|n| n == id), &mut out),
        "cursor" => {
            let named = |p: &Path| p.file_name().is_some_and(|n| n == id || n.to_string_lossy() == format!("{id}.jsonl") || n.to_string_lossy() == format!("{id}.txt"));
            for chats in [crate::usage::cursor::config_home().join("chats"), crate::usage::cursor::data_home().join("chats")] {
                find(&chats, 1, &|p| p.is_dir() && p.file_name().is_some_and(|n| n == id), &mut out);
            }
            let projects = crate::usage::cursor::data_home().join("projects");
            for project in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
                find(&project.path().join("agent-transcripts"), 0, &named, &mut out);
            }
        }
        "antigravity" => {
            let base = crate::usage::antigravity::base();
            out.push(base.join("brain").join(id));
            for ext in ["db", "db-wal", "db-shm"] {
                out.push(base.join("conversations").join(format!("{id}.{ext}")));
            }
            out.retain(|p| p.exists());
        }
        "omp" => {
            let suffix = format!("_{id}.jsonl");
            find(&crate::usage::local::omp_home().join("agent/sessions"), 1, &|p| p.file_name().is_some_and(|n| n.to_string_lossy().ends_with(&suffix)), &mut out);
        }
        _ => {}
    }
    out.sort();
    out
}

/// Text without terminal escapes (the renderers colour for the terminal) or other
/// control characters, lines kept.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // CSI: ESC [ parameters final-byte; OSC: ESC ] … BEL or ESC \.
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
        } else if c == '\n' || c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    out
}

/// The text of a content field: a string, or the text blocks of a list.
fn content_text(c: &serde_json::Value) -> String {
    match c {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            blocks.iter().filter_map(|b| b.as_str().or(b["text"].as_str())).collect::<Vec<_>>().join(" ")
        }
        _ => String::new(),
    }
}

/// A transcript in one of the common shapes (`{role, content}`, `{message: {role,
/// content}}`, Antigravity's `{type: USER_INPUT, content}`) as "you:" and "agent:"
/// lines.
fn generic_transcript(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let m = if v["message"].is_object() { &v["message"] } else { &v };
        let role = m["role"].as_str().or(v["role"].as_str()).or(v["type"].as_str()).unwrap_or("");
        let who = match role {
            "user" | "USER_INPUT" => "you",
            "assistant" | "PLANNER_RESPONSE" => "agent",
            _ => continue,
        };
        let text = content_text(if m["content"].is_null() { &v["content"] } else { &m["content"] });
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let short: String = text.chars().take(600).collect();
        out.push_str(&format!("{who}: {short}\n\n"));
    }
    out
}

/// Muse's records (each line, and the records nested in its `children`): the
/// prompts that started runs and the replies committed.
fn muse_transcript(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let Ok(o) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let mut recs = vec![];
        if o.get("payload_type").is_some() {
            recs.push(o.clone());
        }
        for c in o["children"].as_array().into_iter().flatten() {
            if let Some(r) = c["record_json"].as_str().and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()) {
                recs.push(r);
            }
        }
        for r in recs.iter().filter(|r| r["payload_type"] == "runtime.session") {
            let e = &r["payload"]["event"];
            let (who, text) = match e["kind"].as_str() {
                Some("started") => ("you", e["prompt"].as_str()),
                Some("assistant_message_committed") => ("agent", e["text"].as_str()),
                _ => continue,
            };
            if let Some(t) = text.map(str::trim).filter(|t| !t.is_empty()) {
                out.push_str(&format!("{who}: {}\n\n", t.chars().take(600).collect::<String>()));
            }
        }
    }
    out
}

/// The transcript file of a session, for the agents read generically.
fn transcript_file(harness: &str, files: &[PathBuf]) -> Option<PathBuf> {
    let first_file = |name: &str| files.iter().map(|f| f.join(name)).find(|p| p.is_file());
    match harness {
        "grok" => first_file("chat_history.jsonl"),
        "muse" => first_file("session.jsonl"),
        "antigravity" => first_file(".system_generated/logs/transcript.jsonl"),
        "omp" => files.iter().find(|f| f.is_file()).cloned(),
        "cursor" => files
            .iter()
            .flat_map(|f| if f.is_dir() { std::fs::read_dir(f).into_iter().flatten().flatten().map(|e| e.path()).collect() } else { vec![f.clone()] })
            .find(|p| p.extension().is_some_and(|x| x == "jsonl") && p.parent().is_some_and(|d| d.ends_with("agent-transcripts") || d.parent().is_some_and(|g| g.ends_with("agent-transcripts")))),
        _ => None,
    }
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
        _ => {
            let all = records(harness, id);
            let body = std::fs::read_to_string(transcript_file(harness, &all)?).ok()?;
            if harness == "muse" { muse_transcript(&body) } else { generic_transcript(&body) }
        }
    };
    let text = plain(&text);
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
    fn reads_common_transcript_shapes() {
        let body = [
            r#"{"type":"system","content":"You are an agent"}"#,
            r#"{"role":"user","content":"fix the header"}"#,
            r#"{"type":"message","message":{"role":"assistant","content":[{"type":"text","text":"Done."},{"type":"toolCall"}]}}"#,
            r#"{"type":"USER_INPUT","content":"and the footer"}"#,
        ]
        .join("\n");
        assert_eq!(generic_transcript(&body), "you: fix the header\n\nagent: Done.\n\nyou: and the footer\n\n");
        assert!(trashable("grok") && !trashable("opencode"));
    }

    #[test]
    fn peeks_as_plain_text() {
        assert_eq!(plain("\x1b[36m●\x1b[0m \x1b[1mBash\x1b[0m\n\x1b]8;;x\x07ok\x07"), "● Bash\nok");
    }

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
