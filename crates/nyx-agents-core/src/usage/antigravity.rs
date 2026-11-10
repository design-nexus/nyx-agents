//! Antigravity CLI: `~/.gemini/antigravity-cli` (`ANTIGRAVITY_DATA_DIR` overrides):
//! `history.jsonl` (one line per prompt), `brain/<id>/.system_generated/logs/
//! transcript.jsonl` (models and tools), `conversations/<id>.db` (steps, and today's
//! tokens in each step's metadata), `conversation_summaries.db` (titles) and
//! `presence/<id>.lock` (held while a session runs). The quota comes from
//! `agy -p /usage`.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use nyx_agents_proto::{QuotaWindow, Usage, UsageDay, UsageSession};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logcache::LogCache;
use super::{Tally, json_line, quota, text, time};

const VERSION: u32 = 1;

pub fn base() -> PathBuf {
    std::env::var_os("ANTIGRAVITY_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".gemini/antigravity-cli"))
}

fn configured_model(base: &Path) -> String {
    std::fs::read(base.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["model"].as_str().map(|m| crate::sanitize::line(m, 80)))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "Gemini".into())
}

/// What one transcript adds up to: the model it ended on, steps and prompts by day.
#[derive(Default, Serialize, Deserialize)]
struct Transcript {
    model: Option<String>,
    steps: HashMap<String, u64>,
    prompts: HashMap<String, u64>,
    tools: HashMap<String, u64>,
}

/// "…`Model Selection` from X to Y. No need…" → Y.
fn model_switch(content: &str) -> Option<String> {
    let i = content.find("Model Selection` from ")?;
    let rest = &content[i..];
    let to = rest.find(" to ")? + 4;
    let tail = &rest[to..];
    let end = tail.find(". No need").or_else(|| tail.rfind('.')).unwrap_or(tail.len());
    let m = crate::sanitize::line(&tail[..end].replace('`', ""), 80);
    (!m.is_empty() && m.len() < 60 && !m.to_lowercase().starts_with("comment")).then_some(m)
}

fn read_line(e: &mut Transcript, line: &[u8], mtime: f64) {
    let Some(step) = json_line(line) else { return };
    let day = time::local_day(time::stamp(&step["created_at"]).unwrap_or(mtime));
    if let Some(m) = step["content"].as_str().and_then(model_switch) {
        e.model = Some(m);
    }
    *e.steps.entry(day.clone()).or_default() += 1;
    if step["type"] == "USER_INPUT" {
        *e.prompts.entry(day).or_default() += 1;
    }
    for tc in step["tool_calls"].as_array().into_iter().flatten() {
        let name = crate::sanitize::line(tc["function"]["name"].as_str().or(tc["name"].as_str()).unwrap_or(""), 80);
        if !name.is_empty() {
            *e.tools.entry(name).or_default() += 1;
        }
    }
}

fn open_ro(p: &Path) -> Option<Connection> {
    let db = Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let _ = db.busy_timeout(std::time::Duration::from_millis(300));
    Some(db)
}

/// One protobuf message's fields; nested messages stay bytes.
fn pb_fields(buf: &[u8]) -> Vec<(u64, PbValue<'_>)> {
    let mut out = vec![];
    let mut i = 0;
    let varint = |i: &mut usize| -> Option<u64> {
        let (mut shift, mut v) = (0, 0u64);
        loop {
            let b = *buf.get(*i)?;
            *i += 1;
            v |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    };
    while i < buf.len() {
        let Some(key) = varint(&mut i) else { break };
        let (field, wire) = (key >> 3, key & 7);
        match wire {
            0 => match varint(&mut i) {
                Some(v) => out.push((field, PbValue::Int(v))),
                None => break,
            },
            2 => {
                let Some(len) = varint(&mut i) else { break };
                let end = i.saturating_add(len as usize);
                let Some(b) = buf.get(i..end) else { break };
                out.push((field, PbValue::Bytes(b)));
                i = end;
            }
            1 => i += 8,
            5 => i += 4,
            _ => break,
        }
    }
    out
}

enum PbValue<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

fn pb_get<'a>(fields: &'a [(u64, PbValue<'a>)], f: u64) -> Option<&'a PbValue<'a>> {
    fields.iter().rev().find(|(k, _)| *k == f).map(|(_, v)| v)
}

/// Today's tokens from the step metadata of conversations written today: field 1 is
/// when the step ran (seconds in its field 1), field 9 its usage (2 uncached input,
/// 5 cached input, 3 output). Returns (uncached input + output, cached input).
fn today_tokens(conversations: &Path, midnight: f64) -> (u64, u64) {
    let (mut total, mut cached) = (0, 0);
    for db in std::fs::read_dir(conversations).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "db")) {
        let wal = db.with_extension("db-wal");
        let m = |p: &Path| std::fs::metadata(p).map(|m| m.mtime() as f64).unwrap_or(0.0);
        if m(&db).max(m(&wal)) < midnight {
            continue;
        }
        let Some(conn) = open_ro(&db) else { continue };
        let Ok(mut st) = conn.prepare("SELECT metadata FROM steps WHERE metadata IS NOT NULL") else { continue };
        let rows = st.query_map([], |r| r.get::<_, Vec<u8>>(0));
        for meta in rows.into_iter().flatten().flatten() {
            let f = pb_fields(&meta);
            let (Some(PbValue::Bytes(when)), Some(PbValue::Bytes(usage))) = (pb_get(&f, 1), pb_get(&f, 9)) else { continue };
            let w = pb_fields(when);
            if !matches!(pb_get(&w, 1), Some(PbValue::Int(t)) if (*t as f64) >= midnight) {
                continue;
            }
            let u = pb_fields(usage);
            for (k, v) in &u {
                if let PbValue::Int(n) = v {
                    match k {
                        2 | 3 => total += n,
                        5 => cached += n,
                        _ => {}
                    }
                }
            }
        }
    }
    (total, cached)
}

/// Sessions whose presence lock a process holds (FLOCK WRITE in /proc/locks).
fn running(presence: &Path) -> HashSet<String> {
    let held: HashSet<(u64, u64)> = std::fs::read_to_string("/proc/locks")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            if p.len() < 6 || p[1] != "FLOCK" || p[3] != "WRITE" {
                return None;
            }
            let d: Vec<&str> = p[5].split(':').collect();
            (d.len() == 3).then(|| {
                let dev = libc::makedev(u32::from_str_radix(d[0], 16).unwrap_or(0), u32::from_str_radix(d[1], 16).unwrap_or(0));
                (dev, d[2].parse().unwrap_or(0))
            })
        })
        .collect();
    std::fs::read_dir(presence)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "lock"))
        .filter(|e| e.metadata().is_ok_and(|m| held.contains(&(m.dev(), m.ino()))))
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect()
}

/// A running session is working while its last step is in progress, or its
/// transcript ends on the user's input or a tool call.
fn working(base: &Path, id: &str) -> bool {
    if let Some(db) = open_ro(&base.join("conversations").join(format!("{id}.db")))
        && let Ok(status) = db.query_row("SELECT status FROM steps ORDER BY idx DESC LIMIT 1", [], |r| r.get::<_, i64>(0))
        && status == 2
    {
        return true;
    }
    let t = base.join("brain").join(id).join(".system_generated/logs/transcript.jsonl");
    let Ok(body) = std::fs::read(&t) else { return false };
    let tail = String::from_utf8_lossy(&body[body.len().saturating_sub(4096)..]).into_owned();
    let Some(last) = tail.lines().rev().find(|l| !l.trim().is_empty()).and_then(|l| serde_json::from_str::<Value>(l).ok()) else { return false };
    matches!(last["type"].as_str(), Some("USER_INPUT" | "GENERIC")) || (last["type"] == "PLANNER_RESPONSE" && last["tool_calls"].as_array().is_some_and(|a| !a.is_empty()))
}

#[derive(Default)]
struct Conv {
    first: String,
    last: String,
    cwd: String,
    at: f64,
    steps: u64,
    prompts: u64,
}

pub fn scan(base: &Path, refresh: bool) -> (Usage, Vec<UsageSession>) {
    let mut u = Usage { harness: "antigravity".into(), name: "Antigravity".into(), ..Default::default() };
    if !base.is_dir() {
        return (u, vec![]);
    }
    let days = time::last_days(7);
    let today = days[6].clone();
    let default_model = configured_model(base);
    let mut convs: HashMap<String, Conv> = HashMap::new();
    // Prompts by day, from the history.
    let mut daily_prompts: HashMap<String, u64> = HashMap::new();
    let mut total_prompts = 0;
    if let Ok(body) = std::fs::read_to_string(base.join("history.jsonl")) {
        for l in body.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(e) = serde_json::from_str::<Value>(l) else { continue };
            total_prompts += 1;
            let at = time::stamp(&e["timestamp"]).unwrap_or(0.0);
            *daily_prompts.entry(time::local_day(if at > 0.0 { at } else { time::now() })).or_default() += 1;
            let id = text(&e["conversationId"], 100);
            if id.is_empty() {
                continue;
            }
            let display = text(&e["display"], 200);
            let c = convs.entry(id).or_default();
            if c.first.is_empty() {
                c.first = display.clone();
            }
            if !display.is_empty() {
                c.last = display;
            }
            c.at = c.at.max(at);
            c.prompts += 1;
            let ws = text(&e["workspace"], 300);
            if !ws.is_empty() {
                c.cwd = ws;
            }
        }
    }
    // Models and tools, from the transcripts (each counted under the model it ended on).
    let mut t = Tally::default();
    let mut cache = LogCache::new("antigravity", VERSION);
    let mut transcripts: Vec<(PathBuf, f64)> = std::fs::read_dir(base.join("brain"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join(".system_generated/logs/transcript.jsonl"))
        .filter(|p| p.is_file())
        .map(|p| {
            let m = std::fs::metadata(&p).map(|m| m.mtime() as f64).unwrap_or(0.0);
            (p, m)
        })
        .collect();
    transcripts.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut latest_model = None;
    for (p, _) in &transcripts {
        let Some(rec) = cache.update(p, read_line) else { continue };
        let e = rec.entry;
        let model = e.model.clone().unwrap_or_else(|| default_model.clone());
        latest_model.get_or_insert_with(|| model.clone());
        for (d, n) in &e.prompts {
            t.prompt(&model, d, *n);
        }
        for (d, n) in &e.steps {
            t.step(&model, d, *n);
        }
        for (name, n) in &e.tools {
            t.tool(name, *n);
        }
    }
    cache.prune();
    // Steps and when each conversation last changed, from its database.
    for db in std::fs::read_dir(base.join("conversations")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "db")) {
        let id = db.file_stem().map(|s| crate::sanitize::line(&s.to_string_lossy(), 100)).unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let mtime = std::fs::metadata(&db).map(|m| m.mtime() as f64).unwrap_or(0.0);
        let steps = open_ro(&db).and_then(|c| c.query_row("SELECT count(*) FROM steps", [], |r| r.get::<_, i64>(0)).ok()).unwrap_or(0).max(0) as u64;
        let c = convs.entry(id.clone()).or_default();
        if c.first.is_empty() {
            c.first = format!("Session {}", &id[..id.len().min(8)]);
        }
        c.steps = c.steps.max(steps);
        c.at = c.at.max(mtime);
    }
    // Titles from the summaries, when Antigravity keeps them.
    if let Some(db) = open_ro(&base.join("conversation_summaries.db"))
        && let Ok(mut st) = db.prepare("SELECT conversation_id, title, preview, step_count, workspace_uris FROM conversation_summaries")
    {
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<String>>(4)?))
        });
        for (id, title, preview, steps, ws) in rows.into_iter().flatten().flatten() {
            let Some(id) = id.filter(|i| !i.is_empty()) else { continue };
            let c = convs.entry(id).or_default();
            if let Some(t) = title.filter(|t| !t.is_empty()) {
                c.first = crate::sanitize::line(&t, 150);
            }
            if let Some(p) = preview.filter(|p| !p.is_empty()) {
                c.last = crate::sanitize::line(&p, 250);
            }
            c.steps = c.steps.max(steps.unwrap_or(0).max(0) as u64);
            if c.cwd.is_empty() {
                c.cwd = ws.unwrap_or_default();
            }
        }
    }
    let live = running(&base.join("presence"));
    for id in &live {
        convs.entry(id.clone()).or_insert_with(|| Conv { first: format!("Session {}", &id[..id.len().min(8)]), at: time::now(), ..Default::default() });
    }
    let steps_of: HashMap<String, u64> = convs.iter().map(|(id, c)| (id.clone(), c.steps)).collect();
    let mut sessions: Vec<UsageSession> = convs
        .into_iter()
        .map(|(id, c)| {
            let title = if !c.first.is_empty() { c.first } else if !c.last.is_empty() { c.last } else { format!("Session {}", &id[..id.len().min(8)]) };
            UsageSession { active: live.contains(&id), id, title, cwd: c.cwd, updated: c.at as u64, prompts: c.prompts, ..Default::default() }
        })
        .collect();
    sessions.sort_by(|a, b| b.active.cmp(&a.active).then(b.updated.cmp(&a.updated)).then(a.id.cmp(&b.id)));
    u.working = live.iter().any(|id| working(base, id));
    u.has_records = !sessions.is_empty();
    u.last_used = sessions.iter().map(|s| s.updated).max();
    u.model = latest_model.or(Some(default_model));
    t.finish(&mut u);
    // The day counts follow the old hub: prompts from the history, steps from each
    // conversation's database by the day it last changed.
    let model_steps_today = u.today.steps;
    let mut day_steps: HashMap<String, u64> = HashMap::new();
    for s in &sessions {
        *day_steps.entry(time::local_day(s.updated as f64)).or_default() += steps_of.get(&s.id).copied().unwrap_or(0);
    }
    u.days = days.iter().map(|d| UsageDay { date: d.clone(), prompts: daily_prompts.get(d).copied().unwrap_or(0), steps: day_steps.get(d).copied().unwrap_or(0) }).collect();
    u.today.prompts = daily_prompts.get(&today).copied().unwrap_or(0);
    u.today.steps = if model_steps_today > 0 { model_steps_today } else { day_steps.get(&today).copied().unwrap_or(0) };
    u.today.sessions = sessions.iter().filter(|s| time::local_day(s.updated as f64) == today).count() as u64;
    u.totals.prompts = total_prompts;
    u.totals.sessions = sessions.len() as u64;
    if u.totals.steps == 0 {
        u.totals.steps = steps_of.values().sum();
    }
    let midnight = {
        let now = time::now();
        let tm = {
            let t = now as libc::time_t;
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            unsafe { libc::localtime_r(&t, &mut tm) };
            tm
        };
        now - (tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec) as f64
    };
    let (tok, cached) = today_tokens(&base.join("conversations"), midnight);
    u.today.tokens = tok;
    u.today.cached = cached;
    sessions.truncate(super::HISTORY);
    u.sessions = sessions.iter().take(10).cloned().collect();
    u.quota = quota::get("antigravity", refresh, 180.0, fetch_quota);
    (u, sessions)
}

/// Each model group's 5-hour and weekly windows, from `agy -p /usage`.
fn fetch_quota() -> quota::Fetched {
    let exe = crate::adapters::exe("antigravity").ok_or("Antigravity isn't installed")?;
    let mut child = std::process::Command::new(exe)
        .args(["-p", "/usage", "--output-format", "json"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Couldn't start Antigravity: {e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(100)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Antigravity didn't answer in time".into());
            }
        }
    };
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        use std::io::Read;
        let _ = so.read_to_string(&mut out);
    }
    if !status.success() {
        return Err("Antigravity couldn't report its usage".into());
    }
    let data: Value = serde_json::from_str(&out).map_err(|_| "Antigravity's answer wasn't JSON")?;
    let groups = data["command"]["data"]["groups"].as_array().cloned().unwrap_or_default();
    let many = groups.len() > 1;
    let mut windows = vec![];
    for g in &groups {
        let group = crate::sanitize::line(g["name"].as_str().unwrap_or("Models"), 40);
        let mut ws: Vec<QuotaWindow> = g["buckets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| {
                let win = format!("{} {}", b["window"].as_str().unwrap_or(""), b["name"].as_str().unwrap_or("")).to_lowercase();
                let span = if win.contains("weekly") { "Week" } else { "5 hours" };
                QuotaWindow {
                    id: crate::sanitize::line(b["id"].as_str().unwrap_or(span), 50),
                    label: if many { format!("{group}, {span}") } else { span.to_string() },
                    used_percent: 100.0 - (b["remaining_fraction"].as_f64().unwrap_or(1.0).clamp(0.0, 1.0) * 100.0),
                    resets_at: b["reset_time"].as_str().and_then(time::parse_iso).map(|t| t as u64),
                    ..Default::default()
                }
            })
            .collect();
        ws.sort_by_key(|w| w.label.ends_with("Week"));
        windows.extend(ws);
    }
    Ok((None, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_model_switches() {
        assert_eq!(model_switch("Changed `Model Selection` from Gemini Flash to `Gemini 3 Pro (High)`. No need to reply.").as_deref(), Some("Gemini 3 Pro (High)"));
        assert_eq!(model_switch("nothing here"), None);
    }

    #[test]
    fn reads_protobuf_fields() {
        // field 1 varint 150, field 9 bytes [0x10, 0x05]
        let f = pb_fields(&[0x08, 0x96, 0x01, 0x4a, 0x02, 0x10, 0x05]);
        assert!(matches!(pb_get(&f, 1), Some(PbValue::Int(150))));
        let Some(PbValue::Bytes(b)) = pb_get(&f, 9) else { panic!() };
        assert!(matches!(pb_get(&pb_fields(b), 2), Some(PbValue::Int(5))));
    }
}
