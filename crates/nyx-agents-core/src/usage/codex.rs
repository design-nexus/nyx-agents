//! Codex CLI: rollouts under `~/.codex/sessions` and `archived_sessions` from the
//! last 30 days (`CODEX_HOME` overrides the folder), titles from
//! `session_index.jsonl`, and the quota from `codex app-server`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use nyx_agents_proto::{QuotaWindow, Usage, UsageSession};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::logcache::LogCache;
use super::{Tally, json_line, quota, text, time};

const VERSION: u32 = 1;
const KEEP_DAYS: f64 = 30.0;

pub fn base() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".codex"))
}

/// The model set at the top of `config.toml`, if any.
fn configured_model(base: &Path) -> Option<String> {
    let text = std::fs::read_to_string(base.join("config.toml")).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table.get("model").and_then(|m| m.as_str()).map(|m| crate::sanitize::line(m, 60)).filter(|m| !m.is_empty())
}

/// What one rollout adds up to. `counts`: by model and day, (prompts, steps,
/// input, output, cached input). A prompt is a turn (`task_started`); a step is a
/// model call (`token_count`).
#[derive(Default, Serialize, Deserialize)]
struct Rollout {
    sid: String,
    cwd: String,
    model: String,
    prompts: u64,
    counts: HashMap<String, [u64; 5]>,
    tools: HashMap<String, u64>,
}

fn read_line(e: &mut Rollout, line: &[u8], mtime: f64, default_model: &str) {
    let Some(row) = json_line(line) else { return };
    if e.model.is_empty() {
        e.model = default_model.to_string();
    }
    let kind = row["type"].as_str().unwrap_or("");
    let p = &row["payload"];
    let ptype = p["type"].as_str().unwrap_or("");
    match kind {
        "session_meta" => {
            e.sid = text(if p["id"].is_null() { &p["session_id"] } else { &p["id"] }, 100);
            e.cwd = p["cwd"].as_str().unwrap_or("").to_string();
        }
        "turn_context" => {
            let m = if p["model"].is_null() { &p["model_slug"] } else { &p["model"] };
            let m = text(m, 60);
            if !m.is_empty() {
                e.model = m;
            }
        }
        "response_item" if matches!(ptype, "function_call" | "custom_tool_call") => {
            let name = text(&p["name"], 50);
            if !name.is_empty() {
                *e.tools.entry(name).or_default() += 1;
            }
        }
        "event_msg" if matches!(ptype, "task_started" | "token_count") => {
            let day = time::local_day(time::stamp(&row["timestamp"]).unwrap_or(mtime));
            let c = e.counts.entry(format!("{}\x1f{day}", e.model)).or_default();
            if ptype == "task_started" {
                c[0] += 1;
                e.prompts += 1;
                return;
            }
            let u = &p["info"]["last_token_usage"];
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            let (input, output) = (n("input_tokens"), n("output_tokens"));
            if input + output > 0 {
                c[1] += 1;
                c[2] += input;
                c[3] += output;
                c[4] += n("cached_input_tokens");
            }
        }
        _ => {}
    }
}

/// Sessions in `session_index.jsonl`, by id: their title and when they last changed.
fn index(base: &Path) -> HashMap<String, (String, u64)> {
    let Ok(f) = std::fs::File::open(base.join("session_index.jsonl")) else { return HashMap::new() };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .filter_map(|v| {
            let id = text(&v["id"], 100);
            let title = text(&v["thread_name"], 150);
            let updated = v["updated_at"].as_str().and_then(time::parse_iso).unwrap_or(0.0) as u64;
            (!id.is_empty()).then_some((id, (title, updated)))
        })
        .collect()
}

fn rollouts(base: &Path, since: f64) -> Vec<(PathBuf, f64)> {
    fn walk(dir: &Path, since: f64, out: &mut Vec<(PathBuf, f64)>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                walk(&p, since, out);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64());
                if mtime >= since {
                    out.push((p, mtime));
                }
            }
        }
    }
    let mut out = vec![];
    for d in ["sessions", "archived_sessions"] {
        walk(&base.join(d), since, &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Codex processes that are not a session: the ChatGPT desktop app's own codex, and
/// the app-server, exec-server and MCP daemons.
fn is_session(argv: &[String]) -> bool {
    let exe = argv.first().map(String::as_str).unwrap_or("");
    !(exe.starts_with("/usr/lib/chatgpt/") || exe.contains("/app-server-daemon/"))
        && !argv.iter().skip(1).any(|a| matches!(a.as_str(), "app-server" | "exec-server" | "mcp-server" | "mcp"))
}

/// Running sessions: by conversation id when the command line names one, else by
/// folder.
fn running() -> (Vec<String>, Vec<String>) {
    let (mut ids, mut cwds) = (vec![], vec![]);
    for p in super::procs::named("codex").into_iter().filter(|p| is_session(&p.argv)) {
        match p.argv.iter().find_map(|a| find_uuid(a)) {
            Some(id) => ids.push(id),
            None => cwds.extend(p.cwd),
        }
    }
    (ids, cwds)
}

pub(crate) fn find_uuid(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let shape = [8, 4, 4, 4, 12];
    (0..b.len()).find_map(|start| {
        let mut i = start;
        for (k, len) in shape.iter().enumerate() {
            if k > 0 {
                if b.get(i) != Some(&b'-') {
                    return None;
                }
                i += 1;
            }
            if b.len() < i + len || !b[i..i + len].iter().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
                return None;
            }
            i += len;
        }
        Some(s[start..i].to_string())
    })
}

/// The usage, and every session found (newest first, at most 50).
pub fn scan(base: &Path, refresh: bool) -> (Usage, Vec<UsageSession>) {
    let mut u = Usage { harness: "codex".into(), name: "Codex".into(), ..Default::default() };
    let default_model = configured_model(base).unwrap_or_else(|| "Codex".into());
    let mut t = Tally::default();
    let mut cache = LogCache::new("codex", VERSION);
    let index = index(base);
    // Resumed sessions write more rollouts under the same id.
    let mut by_sid: HashMap<String, UsageSession> = HashMap::new();
    for (path, _) in rollouts(base, time::now() - KEEP_DAYS * 86400.0) {
        let Some(rec) = cache.update(&path, |e: &mut Rollout, line, mtime| read_line(e, line, mtime, &default_model)) else { continue };
        let e = rec.entry;
        for (key, c) in &e.counts {
            let Some((model, day)) = key.split_once('\x1f') else { continue };
            if c[0] > 0 {
                t.prompt(model, day, c[0]);
            }
            if c[1] > 0 {
                t.step(model, day, c[1]);
                t.tokens(model, day, c[2], c[3], c[4], 0);
            }
        }
        for (name, n) in &e.tools {
            t.tool(name, *n);
        }
        let sid = if e.sid.is_empty() { path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() } else { e.sid };
        let s = by_sid.entry(sid.clone()).or_insert_with(|| UsageSession {
            title: index.get(&sid).map(|(t, _)| t.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "Codex session".into()),
            id: sid,
            ..Default::default()
        });
        s.prompts += e.prompts;
        s.updated = s.updated.max(rec.mtime as u64);
        if s.cwd.is_empty() {
            s.cwd = e.cwd;
        }
        if !e.model.is_empty() {
            s.model = Some(e.model);
        }
    }
    cache.prune();
    let recent = by_sid.len() as u64;
    // Older sessions have no rollout read here, but are still worth listing.
    for (id, (title, updated)) in index {
        by_sid.entry(id.clone()).or_insert_with(|| UsageSession {
            id,
            title: if title.is_empty() { "Codex session".into() } else { title },
            updated,
            ..Default::default()
        });
    }
    let mut sessions: Vec<UsageSession> = by_sid.into_values().collect();
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    let (ids, cwds) = running();
    for s in sessions.iter_mut().filter(|s| ids.contains(&s.id)) {
        s.active = true;
    }
    for cwd in &cwds {
        if let Some(s) = sessions.iter_mut().find(|s| !s.active && &s.cwd == cwd) {
            s.active = true;
        }
    }
    u.working = !ids.is_empty() || !cwds.is_empty();
    u.has_records = !sessions.is_empty();
    u.last_used = sessions.first().map(|s| s.updated);
    u.model = configured_model(base).or_else(|| sessions.first().and_then(|s| s.model.clone()));
    u.today.sessions = sessions.iter().filter(|s| t.is_today(&time::local_day(s.updated as f64))).count() as u64;
    u.totals.sessions = recent;
    t.finish(&mut u);
    sessions.truncate(super::HISTORY);
    u.sessions = sessions.iter().take(10).cloned().collect();
    u.quota = quota::get("codex", refresh, 180.0, fetch_quota);
    (u, sessions)
}

/// Lines from a child's stdout, so each reply can wait with a time limit.
struct Rpc {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Rpc {
    fn send(&mut self, v: Value) -> Result<(), String> {
        writeln!(self.stdin, "{v}").and_then(|_| self.stdin.flush()).map_err(|e| e.to_string())
    }

    fn call(&mut self, id: u64, method: &str, params: Value, wait: Duration) -> Result<Value, String> {
        self.send(json!({ "id": id, "method": method, "params": params }))?;
        let deadline = std::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let line = self.lines.recv_timeout(left).map_err(|_| format!("no answer to {method}"))?;
            if let Ok(v) = serde_json::from_str::<Value>(&line)
                && v["id"] == id
            {
                return Ok(v);
            }
        }
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The primary and secondary windows from `codex app-server` (read-only sandbox;
/// it asks OpenAI with Codex's own sign-in and makes no model calls).
fn fetch_quota() -> quota::Fetched {
    let exe = crate::adapters::exe("codex").ok_or("Codex isn't installed")?;
    let mut child = Command::new(exe)
        .args(["-s", "read-only", "-a", "on-request", "app-server"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start Codex: {e}"))?;
    let stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(l).is_err() {
                break;
            }
        }
    });
    let mut rpc = Rpc { child, stdin, lines };
    let err = |e: String| format!("Couldn't get Codex's quota: {e}");
    rpc.call(1, "initialize", json!({ "clientInfo": { "name": "nyx-agents", "version": "1" } }), Duration::from_secs(6)).map_err(err)?;
    rpc.send(json!({ "method": "initialized", "params": {} })).map_err(err)?;
    let account = rpc.call(2, "account/read", json!({}), Duration::from_secs(4)).map_err(err)?;
    let limits = rpc.call(3, "account/rateLimits/read", json!({}), Duration::from_secs(4)).map_err(err)?;
    let rl = &limits["result"]["rateLimits"];
    let acct = &account["result"]["account"];
    let plan = [&rl["planType"], &acct["planType"], &acct["type"]].iter().find_map(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    });
    let windows = ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let w = &rl[*key];
            let mins = w["windowDurationMins"].as_u64().unwrap_or(0);
            let label = match mins {
                10080 => "Week".to_string(),
                0 => "Limit".to_string(),
                m if m % 60 == 0 => format!("{} hours", m / 60),
                m => format!("{m} minutes"),
            };
            Some(QuotaWindow {
                id: format!("codex-{key}"),
                label,
                used_percent: w["usedPercent"].as_f64()?.clamp(0.0, 100.0),
                resets_at: w["resetsAt"].as_f64().map(|t| t as u64),
                ..Default::default()
            })
        })
        .collect();
    Ok((plan, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_sessions_from_daemons() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_session(&v(&["/home/k/.local/bin/codex", "resume", "x"])));
        assert!(!is_session(&v(&["/home/k/.local/bin/codex", "app-server"])));
        assert!(!is_session(&v(&["/usr/lib/chatgpt/codex"])));
        assert_eq!(find_uuid("resume 7f3a91c2-58b1-4e0a-9c55-3f1d2a6b9e04").as_deref(), Some("7f3a91c2-58b1-4e0a-9c55-3f1d2a6b9e04"));
        assert_eq!(find_uuid("no id here"), None);
    }
}
