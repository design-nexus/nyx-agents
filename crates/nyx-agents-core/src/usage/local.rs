//! Agents whose usage is only in their own session stores, with no quota to ask
//! about: oh-my-pi (`~/.omp/agent/sessions/<folder>/<time>_<id>.jsonl`), OpenCode
//! (`opencode.db`), Hermes (`~/.hermes/state.db`) and Crush (each project's
//! `.crush/crush.db`, listed in `~/.local/share/crush/projects.json`). A prompt is a
//! user message, a step an assistant one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nyx_agents_proto::{Usage, UsageSession};
use rusqlite::{Connection, OpenFlags, types::Value as Sql};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logcache::LogCache;
use super::{Tally, json_line, text, time};

const VERSION: u32 = 1;

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".local/share"))
}

#[derive(Default)]
struct Session {
    id: String,
    title: String,
    cwd: String,
    updated: f64,
}

/// One message: its session, role, model, time and tokens (input, output, cache
/// read, cache write), and the tool it called, if any.
#[derive(Default)]
struct Msg {
    session: String,
    role: String,
    model: String,
    at: f64,
    tokens: [u64; 4],
    tool: Option<String>,
}

fn assemble(harness: &str, name: &str, comm: &str, mut list: Vec<Session>, msgs: Vec<Msg>) -> (Usage, Vec<UsageSession>) {
    let mut u = Usage { harness: harness.into(), name: name.into(), ..Default::default() };
    let mut t = Tally::default();
    let mut prompts: HashMap<&str, u64> = HashMap::new();
    let mut models: HashMap<&str, String> = HashMap::new();
    let mut last: HashMap<&str, f64> = HashMap::new();
    for m in &msgs {
        let model = if m.model.is_empty() { name.to_string() } else { m.model.clone() };
        let day = time::local_day(m.at);
        match m.role.as_str() {
            "user" => {
                t.prompt(&model, &day, 1);
                *prompts.entry(&m.session).or_default() += 1;
            }
            "assistant" => t.step(&model, &day, 1),
            _ => {}
        }
        if m.tokens.iter().any(|x| *x > 0) {
            t.tokens(&model, &day, m.tokens[0], m.tokens[1], m.tokens[2], m.tokens[3]);
        }
        if let Some(tool) = &m.tool {
            t.tool(tool, 1);
        }
        if !m.model.is_empty() {
            models.insert(&m.session, m.model.clone());
        }
        let l = last.entry(&m.session).or_default();
        *l = l.max(m.at);
    }
    for s in list.iter_mut() {
        if let Some(l) = last.get(s.id.as_str()) {
            s.updated = s.updated.max(*l);
        }
    }
    let mut sessions: Vec<UsageSession> = list
        .iter()
        .map(|s| UsageSession {
            id: s.id.clone(),
            title: if s.title.is_empty() { format!("{name} session") } else { s.title.clone() },
            cwd: s.cwd.clone(),
            updated: s.updated as u64,
            prompts: prompts.get(s.id.as_str()).copied().unwrap_or(0),
            model: models.get(s.id.as_str()).cloned(),
            tokens: 0,
            active: false,
        })
        .collect();
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    let cwds: Vec<String> = super::procs::named(comm).into_iter().filter_map(|p| p.cwd).collect();
    for cwd in &cwds {
        if let Some(s) = sessions.iter_mut().find(|s| !s.active && &s.cwd == cwd) {
            s.active = true;
        }
    }
    u.working = !cwds.is_empty();
    u.has_records = !sessions.is_empty();
    u.last_used = sessions.first().map(|s| s.updated);
    u.model = sessions.iter().find_map(|s| s.model.clone());
    u.today.sessions = sessions.iter().filter(|s| t.is_today(&time::local_day(s.updated as f64))).count() as u64;
    u.totals.sessions = sessions.len() as u64;
    t.finish(&mut u);
    sessions.truncate(super::HISTORY);
    u.sessions = sessions.iter().take(10).cloned().collect();
    (u, sessions)
}

// ---------- oh-my-pi ----------

/// What one oh-my-pi session file adds up to.
#[derive(Default, Serialize, Deserialize)]
struct OmpFile {
    cwd: String,
    title: String,
    model: String,
    msgs: Vec<OmpMsg>,
}

/// A message: its role, model, time, tokens (input, output, cache read, cache write)
/// and the tool it called.
#[derive(Serialize, Deserialize)]
struct OmpMsg {
    role: String,
    model: String,
    at: f64,
    tokens: [u64; 4],
    tool: Option<String>,
}

fn omp_line(e: &mut OmpFile, line: &[u8], mtime: f64) {
    let Some(r) = json_line(line) else { return };
    match r["type"].as_str().unwrap_or("") {
        "session" => e.cwd = r["cwd"].as_str().unwrap_or("").to_string(),
        "title" | "title_change" => {
            let t = text(&r["title"], 150);
            if !t.is_empty() {
                e.title = t;
            }
        }
        "model_change" => e.model = text(&r["model"], 80),
        "message" => {
            let m = &r["message"];
            let role = m["role"].as_str().unwrap_or("").to_string();
            if role != "user" && role != "assistant" {
                return;
            }
            let model = m["model"].as_str().map(|x| crate::sanitize::line(x, 80)).filter(|x| !x.is_empty()).unwrap_or_else(|| e.model.rsplit('/').next().unwrap_or("").to_string());
            let at = time::stamp(&r["timestamp"]).unwrap_or(mtime);
            let n = |k: &str| m["usage"][k].as_u64().unwrap_or(0);
            let tool = m["content"].as_array().and_then(|c| c.iter().find(|b| b["type"] == "toolCall")).and_then(|b| b["name"].as_str()).map(|s| crate::sanitize::line(s, 60));
            e.msgs.push(OmpMsg { role, model, at, tokens: [n("input"), n("output"), n("cacheRead"), n("cacheWrite")], tool });
        }
        _ => {}
    }
}

pub fn omp(refresh: bool) -> (Usage, Vec<UsageSession>) {
    let _ = refresh;
    let base = crate::paths::home().join(".omp/agent/sessions");
    let mut cache = LogCache::new("omp", VERSION);
    let (mut list, mut msgs) = (vec![], vec![]);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|d| std::fs::read_dir(d.path()).into_iter().flatten().flatten().map(|f| f.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    for f in files {
        let Some(rec) = cache.update(&f, omp_line) else { continue };
        let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let id = stem.split_once('_').map_or(stem.clone(), |(_, id)| id.to_string());
        for m in rec.entry.msgs {
            msgs.push(Msg { session: id.clone(), role: m.role, model: m.model, at: m.at, tokens: m.tokens, tool: m.tool });
        }
        list.push(Session { id, title: rec.entry.title, cwd: rec.entry.cwd, updated: rec.mtime });
    }
    cache.prune();
    assemble("omp", "oh-my-pi", "omp", list, msgs)
}

// ---------- SQLite stores ----------

fn open_ro(p: &Path) -> Option<Connection> {
    let c = Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let _ = c.busy_timeout(std::time::Duration::from_millis(250));
    Some(c)
}

/// Every row of a table as column name → value, so a column a version lacks is
/// just missing.
fn rows(c: &Connection, table: &str) -> Vec<HashMap<String, Sql>> {
    let Ok(mut st) = c.prepare(&format!("SELECT * FROM \"{table}\"")) else { return vec![] };
    let names: Vec<String> = st.column_names().into_iter().map(String::from).collect();
    st.query_map([], |r| Ok(names.iter().enumerate().map(|(i, n)| (n.clone(), r.get::<_, Sql>(i).unwrap_or(Sql::Null))).collect()))
        .map(|it| it.flatten().collect())
        .unwrap_or_default()
}

fn s(r: &HashMap<String, Sql>, k: &str) -> String {
    match r.get(k) {
        Some(Sql::Text(t)) => t.clone(),
        Some(Sql::Integer(i)) => i.to_string(),
        _ => String::new(),
    }
}

fn n(r: &HashMap<String, Sql>, k: &str) -> f64 {
    match r.get(k) {
        Some(Sql::Integer(i)) => *i as f64,
        Some(Sql::Real(f)) => *f,
        Some(Sql::Text(t)) => t.parse().ok().or_else(|| time::parse_iso(t)).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Seconds, or milliseconds when it's that large.
fn secs(x: f64) -> f64 {
    if x > 1e11 { x / 1000.0 } else { x }
}

pub fn opencode(_refresh: bool) -> (Usage, Vec<UsageSession>) {
    opencode_at(&data_home().join("opencode/opencode.db"))
}

fn opencode_at(db: &Path) -> (Usage, Vec<UsageSession>) {
    let Some(c) = open_ro(db) else { return assemble("opencode", "OpenCode", "opencode", vec![], vec![]) };
    let list = rows(&c, "session")
        .iter()
        .filter(|r| s(r, "parent_id").is_empty())
        .map(|r| Session { id: s(r, "id"), title: crate::sanitize::line(&s(r, "title"), 150), cwd: s(r, "directory"), updated: secs(n(r, "time_updated")) })
        .collect();
    let msgs = rows(&c, "message")
        .iter()
        .map(|r| {
            let d: Value = serde_json::from_str(&s(r, "data")).unwrap_or(Value::Null);
            let t = &d["tokens"];
            let g = |v: &Value| v.as_u64().unwrap_or(0);
            Msg {
                session: s(r, "session_id"),
                role: d["role"].as_str().unwrap_or("").into(),
                model: text(&d["modelID"], 80),
                at: secs(d["time"]["created"].as_f64().unwrap_or_else(|| n(r, "time_created"))),
                tokens: [g(&t["input"]), g(&t["output"]), g(&t["cache"]["read"]), g(&t["cache"]["write"])],
                tool: None,
            }
        })
        .collect();
    assemble("opencode", "OpenCode", "opencode", list, msgs)
}

pub fn hermes(_refresh: bool) -> (Usage, Vec<UsageSession>) {
    let Some(c) = open_ro(&crate::paths::home().join(".hermes/state.db")) else { return assemble("hermes", "Hermes", "hermes", vec![], vec![]) };
    let sessions = rows(&c, "sessions");
    let model_of: HashMap<String, String> = sessions.iter().map(|r| (s(r, "id"), crate::sanitize::line(&s(r, "model"), 80))).collect();
    let list = sessions
        .iter()
        .map(|r| {
            let title = [s(r, "title"), s(r, "display_name")].into_iter().find(|t| !t.is_empty()).unwrap_or_default();
            let updated = ["updated_at", "ended_at", "started_at", "created_at"].iter().map(|k| secs(n(r, k))).fold(0.0, f64::max);
            Session { id: s(r, "id"), title: crate::sanitize::line(&title, 150), cwd: s(r, "cwd"), updated }
        })
        .collect();
    let msgs = rows(&c, "messages")
        .iter()
        .map(|r| {
            let session = s(r, "session_id");
            Msg {
                model: model_of.get(&session).cloned().unwrap_or_default(),
                session,
                role: s(r, "role"),
                at: secs(n(r, "timestamp")),
                tokens: [0, n(r, "token_count") as u64, 0, 0],
                tool: Some(crate::sanitize::line(&s(r, "tool_name"), 60)).filter(|t| !t.is_empty()),
            }
        })
        .collect();
    assemble("hermes", "Hermes", "hermes", list, msgs)
}

pub fn crush(_refresh: bool) -> (Usage, Vec<UsageSession>) {
    let projects: Value = std::fs::read(data_home().join("crush/projects.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
    let (mut list, mut msgs) = (vec![], vec![]);
    for p in projects["projects"].as_array().into_iter().flatten() {
        let (Some(cwd), Some(dir)) = (p["path"].as_str(), p["data_dir"].as_str()) else { continue };
        let Some(c) = open_ro(&Path::new(dir).join("crush.db")) else { continue };
        for r in rows(&c, "sessions").iter().filter(|r| s(r, "parent_session_id").is_empty()) {
            list.push(Session { id: s(r, "id"), title: crate::sanitize::line(&s(r, "title"), 150), cwd: cwd.to_string(), updated: secs(n(r, "updated_at")) });
        }
        for r in rows(&c, "messages") {
            msgs.push(Msg { session: s(&r, "session_id"), role: s(&r, "role"), model: crate::sanitize::line(&s(&r, "model"), 80), at: secs(n(&r, "created_at")), ..Default::default() });
        }
    }
    assemble("crush", "Crush", "crush", list, msgs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_counts_messages() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("opencode.db");
        let c = Connection::open(&db).unwrap();
        c.execute_batch(
            "CREATE TABLE session (id TEXT, parent_id TEXT, directory TEXT, title TEXT, time_updated INTEGER);
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        let now = (time::now() * 1000.0) as i64;
        c.execute("INSERT INTO session VALUES ('s1', NULL, '/p/site', 'Fix the header', ?1)", [now]).unwrap();
        c.execute("INSERT INTO session VALUES ('s2', 's1', '/p/site', 'subagent', ?1)", [now]).unwrap();
        c.execute("INSERT INTO message VALUES ('m1', 's1', ?1, '{\"role\":\"user\"}')", [now]).unwrap();
        c.execute(
            "INSERT INTO message VALUES ('m2', 's1', ?1, '{\"role\":\"assistant\",\"modelID\":\"qwen3-8b\",\"tokens\":{\"input\":10,\"output\":5,\"cache\":{\"read\":3,\"write\":0}}}')",
            [now],
        )
        .unwrap();
        drop(c);
        let (u, list) = opencode_at(&db);
        assert_eq!(list.len(), 1, "subagent sessions aren't listed");
        assert_eq!(list[0].title, "Fix the header");
        assert_eq!((u.today.prompts, u.today.steps, u.today.tokens, u.today.cached), (1, 1, 15, 3));
        assert_eq!(u.model.as_deref(), Some("qwen3-8b"));
    }
}
