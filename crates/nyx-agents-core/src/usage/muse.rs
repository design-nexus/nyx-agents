//! Muse Code: `~/.local/share/muse` (`MUSE_DATA_HOME` overrides): the session index
//! (`session-index.db`) and each session's `sessions/Y/M/D/<id>/session.jsonl`.
//! Muse reports no quota.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nyx_agents_proto::{Usage, UsageSession};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logcache::LogCache;
use super::{Tally, json_line, time};

const VERSION: u32 = 1;
/// The newest transcripts read (paths are by date, so the cap drops the oldest).
const MAX_TRANSCRIPTS: usize = 400;

pub fn base() -> PathBuf {
    std::env::var_os("MUSE_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".local/share/muse"))
}

/// Muse records time in microseconds.
fn stamp(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|x| *x > 0.0).map(|x| if x > 1e12 { x / 1e6 } else { x }),
        Value::String(s) => time::parse_iso(s),
        _ => None,
    }
}

/// `muse-spark-2` → `Muse Spark 2`.
pub fn model_name(raw: &str) -> String {
    let name = raw.trim();
    if name.to_lowercase().starts_with("muse-") {
        let words: Vec<String> = name[5..]
            .split(['-', ' '])
            .filter(|w| !w.is_empty())
            .map(|w| {
                if w.len() <= 2 {
                    w.to_uppercase()
                } else {
                    let mut c = w.chars();
                    c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
                }
            })
            .collect();
        return format!("Muse {}", words.join(" "));
    }
    crate::sanitize::line(name, 80)
}

fn text_of(blocks: &Value) -> String {
    match blocks {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|b| b.as_str().or(b["text"].as_str())).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// What one transcript adds up to: prompts and steps by local day ("" when a record
/// had no time).
#[derive(Default, Serialize, Deserialize)]
struct Transcript {
    model: String,
    preview: String,
    day_prompts: HashMap<String, u64>,
    day_steps: HashMap<String, u64>,
    tools: HashMap<String, u64>,
}

fn day_of(when: Option<f64>) -> String {
    when.map(time::local_day).unwrap_or_default()
}

fn read_record(e: &mut Transcript, rec: &Value) {
    let p = &rec["payload"];
    let when = stamp(&rec["recorded_at"]);
    match rec["payload_type"].as_str().unwrap_or("") {
        // Only a preview: the matching started event counts the prompt.
        "runtime.user_intent.accepted" => {
            let t = crate::sanitize::line(&text_of(&p["refill_blocks"]), 140);
            if e.preview.is_empty() && !t.is_empty() {
                e.preview = t;
            }
        }
        "run.model.configured" => {
            let r = &p["record"];
            let m = model_name(r["display_label"].as_str().or(r["model_id"].as_str()).unwrap_or(""));
            if !m.is_empty() {
                e.model = m;
            }
        }
        "tool_batch.effect.started" => {
            let tool = crate::sanitize::line(p["record"]["tool_name"].as_str().unwrap_or(""), 60);
            if !tool.is_empty() {
                *e.tools.entry(tool).or_default() += 1;
                *e.day_steps.entry(day_of(when)).or_default() += 1;
            }
        }
        "runtime.session" => match p["event"]["kind"].as_str() {
            Some("started") if p["event"]["prompt"].as_str().is_some_and(|s| !s.trim().is_empty()) => {
                *e.day_prompts.entry(day_of(when)).or_default() += 1;
                if e.preview.is_empty() {
                    e.preview = crate::sanitize::line(p["event"]["prompt"].as_str().unwrap_or(""), 140);
                }
            }
            Some("assistant_message_committed" | "assistant_tool_calls_committed") => {
                *e.day_steps.entry(day_of(when)).or_default() += 1;
            }
            _ => {}
        },
        _ => {}
    }
}

fn read_line(e: &mut Transcript, line: &[u8], _mtime: f64) {
    let Some(outer) = json_line(line) else { return };
    if outer.get("payload_type").is_some() {
        read_record(e, &outer);
    }
    for child in outer["children"].as_array().into_iter().flatten() {
        if let Some(rec) = child["record_json"].as_str().and_then(|r| serde_json::from_str::<Value>(r).ok()).filter(|r| r.get("payload_type").is_some()) {
            read_record(e, &rec);
        }
    }
}

#[derive(Default)]
struct Session {
    title: String,
    preview: String,
    cwd: String,
    model: String,
    updated: f64,
    index_prompts: u64,
    log: Option<PathBuf>,
    has_log: bool,
    day_prompts: HashMap<String, u64>,
    day_steps: HashMap<String, u64>,
}

fn read_index(base: &Path, sessions: &mut HashMap<String, Session>) {
    let Ok(db) = Connection::open_with_flags(base.join("session-index.db"), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else { return };
    let Ok(mut st) = db.prepare(
        "SELECT session_id, title, prompt_count, model_id, workspace_root, created_at_us, updated_at_us, first_user_prompt, session_log_path FROM sessions",
    ) else {
        return;
    };
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, Option<i64>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
        ))
    });
    for (id, title, prompts, model, cwd, created, updated, first, log) in rows.into_iter().flatten().flatten() {
        let Some(id) = id.filter(|i| !i.is_empty()) else { continue };
        let s = sessions.entry(id).or_default();
        let title = crate::sanitize::line(&title.unwrap_or_default(), 140);
        if !title.is_empty() && title != "New session" {
            s.title = title;
        }
        if let Some(c) = cwd.filter(|c| !c.is_empty()) {
            s.cwd = c;
        }
        s.index_prompts = s.index_prompts.max(prompts.unwrap_or(0).max(0) as u64);
        if s.model.is_empty() {
            s.model = model_name(&model.unwrap_or_default());
        }
        if let Some(t) = updated.or(created).and_then(|t| stamp(&Value::from(t))) {
            s.updated = s.updated.max(t);
        }
        if s.preview.is_empty() {
            s.preview = crate::sanitize::line(&first.unwrap_or_default(), 140);
        }
        if let Some(l) = log.filter(|l| !l.is_empty()) {
            s.log = Some(PathBuf::from(l));
        }
    }
}

fn transcripts(base: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let dirs = |p: &Path| -> Vec<PathBuf> { std::fs::read_dir(p).map(|d| d.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()).unwrap_or_default() };
    for y in dirs(&base.join("sessions")) {
        for m in dirs(&y) {
            for d in dirs(&m) {
                for s in dirs(&d) {
                    let f = s.join("session.jsonl");
                    if f.is_file() {
                        out.push(f);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.cmp(a));
    out.truncate(MAX_TRANSCRIPTS);
    out
}

pub fn scan(base: &Path, _refresh: bool) -> (Usage, Vec<UsageSession>) {
    let mut u = Usage { harness: "muse".into(), name: "Muse".into(), ..Default::default() };
    let mut t = Tally::default();
    let mut sessions: HashMap<String, Session> = HashMap::new();
    read_index(base, &mut sessions);
    let mut cache = LogCache::new("muse", VERSION);
    for path in transcripts(base) {
        let id = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(rec) = cache.update(&path, read_line) else { continue };
        let e = rec.entry;
        let s = sessions.entry(id).or_default();
        s.has_log = true;
        if !e.model.is_empty() {
            s.model = e.model;
        }
        if s.preview.is_empty() {
            s.preview = e.preview;
        }
        s.day_prompts = e.day_prompts;
        s.day_steps = e.day_steps;
        s.updated = s.updated.max(rec.mtime);
        for (name, n) in &e.tools {
            t.tool(name, *n);
        }
    }
    cache.prune();
    let today = time::today();
    let mut list = vec![];
    for (id, s) in sessions {
        // The index can lag a deletion: count only sessions whose log remains.
        if !s.has_log && !s.log.as_ref().is_some_and(|l| l.is_file()) {
            continue;
        }
        let logged: u64 = s.day_prompts.values().sum();
        let count = if logged > 0 { logged } else { s.index_prompts };
        let model = if s.model.is_empty() { "Unknown".to_string() } else { s.model.clone() };
        let fallback = if s.updated > 0.0 { time::local_day(s.updated) } else { today.clone() };
        let mut by_day: HashMap<String, (u64, u64)> = HashMap::new();
        for (d, n) in &s.day_prompts {
            by_day.entry(if d.is_empty() { fallback.clone() } else { d.clone() }).or_default().0 += n;
        }
        for (d, n) in &s.day_steps {
            by_day.entry(if d.is_empty() { fallback.clone() } else { d.clone() }).or_default().1 += n;
        }
        if by_day.is_empty() && count > 0 {
            // Known only from the index: every prompt on the session's last day.
            by_day.insert(fallback.clone(), (count, 0));
        }
        for (day, (p, st)) in by_day {
            if p > 0 {
                t.prompt(&model, &day, p);
            }
            // Each prompt took at least one step.
            t.step(&model, &day, st.max(p));
        }
        let title = if !s.title.is_empty() { s.title } else if !s.preview.is_empty() { s.preview } else { "Muse session".into() };
        list.push(UsageSession { id, title, cwd: s.cwd, updated: s.updated as u64, prompts: count, model: Some(s.model).filter(|m| !m.is_empty()), tokens: 0, active: false });
    }
    list.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    let cwds: Vec<String> = super::procs::named("muse").into_iter().filter_map(|p| p.cwd).collect();
    if !cwds.is_empty() {
        match list.iter().position(|s| cwds.contains(&s.cwd)) {
            Some(i) => list[i].active = true,
            None => {
                if let Some(s) = list.first_mut() {
                    s.active = true;
                }
            }
        }
    }
    u.working = !cwds.is_empty();
    u.has_records = !list.is_empty();
    u.last_used = list.first().map(|s| s.updated);
    u.model = list.iter().find_map(|s| s.model.clone());
    u.today.sessions = list.iter().filter(|s| t.is_today(&time::local_day(s.updated as f64))).count() as u64;
    u.totals.sessions = list.len() as u64;
    t.finish(&mut u);
    list.truncate(super::HISTORY);
    u.sessions = list.iter().take(10).cloned().collect();
    (u, list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_models() {
        assert_eq!(model_name("muse-spark-2"), "Muse Spark 2");
        assert_eq!(model_name("muse-pro-xl"), "Muse Pro XL");
    }
}
