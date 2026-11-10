//! Cursor: chats under `~/.config/cursor/chats` and `~/.cursor/chats` (`meta.json`,
//! `prompt_history.json`, `store.db`), agent transcripts under
//! `~/.cursor/projects/*/agent-transcripts`, and the editor's tracking database. The
//! quota comes from Cursor's dashboard API with the sign-in in `auth.json`.
//! (`CURSOR_DATA_HOME` and `CURSOR_CONFIG_HOME` override the folders.)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use nyx_agents_proto::{QuotaWindow, Usage, UsageSession};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logcache::LogCache;
use super::{Tally, json_line, quota, time};

const VERSION: u32 = 1;
/// Prompts whose chat names no model.
const UNRECORDED: &str = "Not recorded";

pub fn data_home() -> PathBuf {
    std::env::var_os("CURSOR_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".cursor"))
}

pub fn config_home() -> PathBuf {
    std::env::var_os("CURSOR_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".config/cursor"))
}

/// Cursor records Auto mode as the model "default".
fn model_name(v: &str) -> String {
    let m = crate::sanitize::line(v, 80);
    if m == "default" { "Auto".into() } else { m }
}

fn clip(s: &str) -> String {
    crate::sanitize::line(s, 140)
}

/// The typed prompt, without the `<timestamp>` and context blocks Cursor wraps around it.
fn prompt_text(content: &Value) -> String {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .filter_map(|b| b.as_str().map(String::from).or_else(|| (b["type"].is_null() || b["type"] == "text").then(|| b["text"].as_str().unwrap_or("").to_string())))
            .collect::<Vec<_>>()
            .join(" "),
        Value::Object(_) => content["text"].as_str().or(content["content"].as_str()).unwrap_or("").to_string(),
        _ => String::new(),
    };
    if let Some(start) = text.find("<user_query>") {
        let rest = &text[start + 12..];
        return clip(rest.split("</user_query>").next().unwrap_or(rest));
    }
    // Drop <tag>…</tag> blocks.
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let name: String = after.chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
        let close = format!("</{name}>");
        match (name.is_empty(), after.find(&close)) {
            (false, Some(j)) => {
                out.push(' ');
                rest = &after[j + close.len()..];
            }
            _ => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    clip(&out)
}

/// What one transcript adds up to. `tokens`: by model and day, (input, output,
/// cache read, cache write).
#[derive(Default, Serialize, Deserialize)]
struct Transcript {
    model: String,
    preview: String,
    prompts: u64,
    steps: u64,
    tokens: HashMap<String, [u64; 4]>,
    tools: HashMap<String, u64>,
}

fn read_line(e: &mut Transcript, line: &[u8], mtime: f64) {
    let Some(obj) = json_line(line) else { return };
    if !obj.is_object() {
        return;
    }
    let message = if obj["message"].is_object() { &obj["message"] } else { &obj };
    let role = obj["role"].as_str().or(message["role"].as_str()).or(obj["type"].as_str()).unwrap_or("");
    let model = message["model"].as_str().or(obj["model"].as_str()).map(model_name).unwrap_or_default();
    if !model.is_empty() {
        e.model = model.clone();
    }
    let usage = if obj["usage"].is_object() { &obj["usage"] } else { &message["usage"] };
    let n = |a: &str, b: &str| usage[a].as_u64().or(usage[b].as_u64()).unwrap_or(0);
    let c = [n("input_tokens", "inputTokens"), n("output_tokens", "outputTokens"), n("cache_read_input_tokens", "cacheReadTokens"), n("cache_creation_input_tokens", "cacheWriteTokens")];
    if c.iter().any(|x| *x > 0) {
        let who = if model.is_empty() { if e.model.is_empty() { "Cursor".to_string() } else { e.model.clone() } } else { model };
        let day = time::local_day(time::stamp(&obj["timestamp"]).unwrap_or(mtime));
        let t = e.tokens.entry(format!("{who}\x1f{day}")).or_default();
        t.iter_mut().zip(c).for_each(|(a, b)| *a += b);
    }
    let content = if message["content"].is_null() { &obj["content"] } else { &message["content"] };
    if let Some(blocks) = content.as_array() {
        for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
            if let Some(name) = b["name"].as_str() {
                *e.tools.entry(crate::sanitize::line(name, 60)).or_default() += 1;
            }
        }
    }
    match role {
        "user" => {
            let p = prompt_text(content);
            if e.preview.is_empty() && !p.is_empty() {
                e.preview = p;
            }
            e.prompts += 1;
        }
        "assistant" => e.steps += 1,
        _ => {}
    }
}

#[derive(Default)]
struct Chat {
    title: String,
    preview: String,
    cwd: String,
    model: String,
    updated: f64,
    prompts: u64,
    steps: u64,
    history_prompts: u64,
    prompt_day: Option<String>,
    has_transcript: bool,
}

fn mtime(p: &Path) -> f64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64())
}

fn dirs(p: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(p).map(|d| d.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect()).unwrap_or_default()
}

fn files(p: &Path, ext: &str) -> Vec<PathBuf> {
    std::fs::read_dir(p)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == ext)).collect())
        .unwrap_or_default()
}

fn read_meta(dir: &Path, chats: &mut HashMap<String, Chat>) {
    let Some(data) = std::fs::read(dir.join("meta.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return };
    if data["isSubagent"] == true {
        return;
    }
    // An empty chat /resume'd elsewhere still logs typed prompts; they belong to the resumed chat.
    if data["hasConversation"] == false && !dir.join("store.db").exists() {
        return;
    }
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let history: Vec<String> = std::fs::read(dir.join("prompt_history.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(String::from))
        // Slash commands such as /quit or /usage never reach a model.
        .filter(|p| !p.trim_start().starts_with('/'))
        .collect();
    let c = chats.entry(id).or_default();
    let title = clip(data["title"].as_str().unwrap_or(""));
    if !title.is_empty() && c.title.is_empty() {
        c.title = title;
    }
    if c.cwd.is_empty() {
        c.cwd = data["cwd"].as_str().unwrap_or("").to_string();
    }
    let updated = time::stamp(&data["updatedAtMs"]).or_else(|| time::stamp(&data["updatedAt"])).or_else(|| time::stamp(&data["createdAtMs"])).unwrap_or_else(|| mtime(&dir.join("meta.json")));
    c.updated = c.updated.max(updated);
    if let Some(first) = history.first().filter(|_| c.preview.is_empty()) {
        c.preview = clip(first);
    }
    c.history_prompts = history.len() as u64;
}

fn open_ro(p: &Path) -> Option<Connection> {
    Connection::open_with_flags(p, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

/// The model picked in a chat's own store (Auto stays Auto: Cursor doesn't report
/// what it routes to), and its title.
fn read_store(p: &Path, c: &mut Chat) {
    let Some(db) = open_ro(p) else { return };
    if let Ok(mut st) = db.prepare("SELECT value FROM meta") {
        let rows = st.query_map([], |r| r.get_ref(0).map(|v| v.as_str().map(String::from).or_else(|_| v.as_blob().map(|b| String::from_utf8_lossy(b).into_owned())).ok()));
        for v in rows.into_iter().flatten().flatten().flatten() {
            let raw = if v.len() % 2 == 0 && v.chars().all(|c| c.is_ascii_hexdigit()) {
                (0..v.len()).step_by(2).filter_map(|i| u8::from_str_radix(&v[i..i + 2], 16).ok()).collect::<Vec<u8>>()
            } else {
                v.into_bytes()
            };
            if let Ok(meta) = serde_json::from_slice::<Value>(&raw)
                && let Some(m) = meta["lastUsedModel"].as_str()
            {
                c.model = model_name(m);
            }
        }
    }
}

/// Titles, models and times from the editor's tracking database.
fn read_tracking(p: &Path, chats: &mut HashMap<String, Chat>) {
    let Some(db) = open_ro(p) else { return };
    if let Ok(mut st) = db.prepare("SELECT conversationId, title, model, updatedAt FROM conversation_summaries") {
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<rusqlite::types::Value>>(3)?))
        });
        for (id, title, model, updated) in rows.into_iter().flatten().flatten() {
            let Some(id) = id.filter(|i| !i.is_empty()) else { continue };
            let c = chats.entry(id).or_default();
            if let Some(t) = title.filter(|_| c.title.is_empty()) {
                c.title = clip(&t);
            }
            if let Some(m) = model.filter(|_| c.model.is_empty()) {
                c.model = model_name(&m);
            }
            let at = match updated {
                Some(rusqlite::types::Value::Integer(i)) => time::stamp(&Value::from(i)),
                Some(rusqlite::types::Value::Real(f)) => time::stamp(&Value::from(f)),
                Some(rusqlite::types::Value::Text(s)) => time::parse_iso(&s).or_else(|| s.parse::<f64>().ok().map(|x| if x > 1e11 { x / 1000.0 } else { x })),
                _ => None,
            };
            if let Some(at) = at {
                c.updated = c.updated.max(at);
            }
        }
    }
}

/// A running agent: node running the cursor-agent package (matched on the process,
/// not on where the command resolves, which under mise is a shim).
fn running() -> bool {
    super::procs::matching(|_| true)
        .iter()
        .any(|p| p.argv.iter().any(|a| a.contains("cursor-agent") && a.ends_with("dist-package/index.js")))
}

pub fn scan(data: &Path, config: &Path, refresh: bool) -> (Usage, Vec<UsageSession>) {
    let mut u = Usage { harness: "cursor".into(), name: "Cursor".into(), ..Default::default() };
    let mut t = Tally::default();
    let mut chats: HashMap<String, Chat> = HashMap::new();
    for root in [config.join("chats"), data.join("chats")] {
        for ws in dirs(&root) {
            for chat in dirs(&ws) {
                read_meta(&chat, &mut chats);
            }
        }
    }
    // A chat whose workspace changes is copied in full to the new project; read the newest copy.
    let mut newest: HashMap<(String, String), PathBuf> = HashMap::new();
    for project in dirs(&data.join("projects")) {
        let at = project.join("agent-transcripts");
        let mut found = vec![];
        for ext in ["jsonl", "txt"] {
            found.extend(files(&at, ext));
            for d in dirs(&at) {
                found.extend(files(&d, ext));
            }
        }
        for p in found {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let parent = p.parent().and_then(|d| d.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let id = if p.extension().is_some_and(|x| x == "jsonl") && stem == parent { parent } else { stem };
            let key = (id, p.extension().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default());
            if newest.get(&key).is_none_or(|old| mtime(&p) > mtime(old)) {
                newest.insert(key, p);
            }
        }
    }
    let jsonl: HashSet<String> = newest.keys().filter(|(_, x)| x == "jsonl").map(|(i, _)| i.clone()).collect();
    let mut cache = LogCache::new("cursor", VERSION);
    let mut keys: Vec<&(String, String)> = newest.keys().collect();
    keys.sort();
    for key in keys {
        let (id, ext) = key;
        let p = &newest[key];
        if ext == "txt" && jsonl.contains(id) {
            continue;
        }
        let c = chats.entry(id.clone()).or_default();
        c.has_transcript = true;
        if ext == "jsonl" {
            let Some(rec) = cache.update(p, read_line) else { continue };
            let e = rec.entry;
            if !e.model.is_empty() {
                c.model = e.model.clone();
            }
            if c.preview.is_empty() {
                c.preview = e.preview.clone();
            }
            c.prompts = e.prompts;
            c.steps = e.steps;
            c.updated = c.updated.max(rec.mtime);
            c.prompt_day = Some(time::local_day(c.updated));
            for (k, v) in &e.tokens {
                if let Some((model, day)) = k.split_once('\x1f') {
                    t.tokens(model, day, v[0], v[1], v[2], v[3]);
                }
            }
            for (name, n) in &e.tools {
                t.tool(name, *n);
            }
        } else if c.prompts == 0 {
            let body = std::fs::read_to_string(p).unwrap_or_default();
            let users: Vec<&str> = body.lines().filter(|l| l.starts_with("user:")).collect();
            c.prompts = users.len() as u64;
            c.updated = c.updated.max(mtime(p));
            c.prompt_day = Some(time::local_day(mtime(p)));
            if c.preview.is_empty()
                && let Some(first) = users.first()
            {
                c.preview = clip(first.split_once(':').map_or("", |x| x.1));
            }
        }
    }
    cache.prune();
    for root in [config.join("chats"), data.join("chats")] {
        for ws in dirs(&root) {
            for chat in dirs(&ws) {
                let id = chat.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if let Some(c) = chats.get_mut(&id) {
                    read_store(&chat.join("store.db"), c);
                }
            }
        }
    }
    read_tracking(&data.join("ai-tracking/ai-code-tracking.db"), &mut chats);
    let today = time::today();
    let mut sessions = vec![];
    for (id, mut c) in chats {
        if c.prompts == 0 && c.history_prompts > 0 {
            c.prompts = c.history_prompts;
        }
        if c.prompts > 0 {
            let model = if c.model.is_empty() { UNRECORDED.to_string() } else { c.model.clone() };
            let day = c.prompt_day.clone().unwrap_or_else(|| if c.updated > 0.0 { time::local_day(c.updated) } else { today.clone() });
            t.prompt(&model, &day, c.prompts);
            t.step(&model, &day, c.steps.max(c.prompts));
        }
        let title = if !c.title.is_empty() { c.title } else if !c.preview.is_empty() { c.preview } else { "Cursor session".into() };
        sessions.push(UsageSession {
            id,
            title,
            cwd: c.cwd,
            updated: c.updated as u64,
            prompts: c.prompts,
            model: Some(c.model).filter(|m| !m.is_empty()),
            tokens: 0,
            active: false,
        });
    }
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated).then(a.id.cmp(&b.id)));
    u.working = running();
    if u.working
        && let Some(s) = sessions.first_mut()
    {
        s.active = true;
    }
    u.has_records = !sessions.is_empty();
    u.last_used = sessions.first().map(|s| s.updated);
    u.model = sessions.iter().find_map(|s| s.model.clone().filter(|m| m != UNRECORDED));
    u.today.sessions = sessions.iter().filter(|s| t.is_today(&time::local_day(s.updated as f64))).count() as u64;
    u.totals.sessions = sessions.len() as u64;
    t.finish(&mut u);
    sessions.truncate(super::HISTORY);
    u.sessions = sessions.iter().take(10).cloned().collect();
    let config = config.to_path_buf();
    u.quota = quota::get("cursor", refresh, 180.0, || fetch_quota(&config));
    (u, sessions)
}

fn post(url: &str, token: &str) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(12))).http_status_as_error(false).build().into();
    let mut resp = agent
        .post(url)
        .header("Authorization", &format!("Bearer {token}"))
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .send("{}")
        .map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200..=299 => {}
        401 | 403 => return Err("the provider didn't accept the sign-in".into()),
        s => return Err(format!("the provider answered {s}")),
    }
    let body = resp.body_mut().with_config().limit(1 << 20).read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|_| "the provider's answer wasn't JSON".into())
}

/// The included allowance, the Auto pool (which a SuperGrok plan shares with Grok),
/// models picked by name, and on-demand spend.
fn fetch_quota(config: &Path) -> quota::Fetched {
    let path = [config.join("auth.json"), crate::paths::home().join(".config/cursor/auth.json")].into_iter().find(|p| p.exists()).ok_or("Cursor isn't signed in")?;
    let auth: Value = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or("Cursor isn't signed in")?;
    let token = auth["accessToken"].as_str().map(str::trim).filter(|t| !t.is_empty()).ok_or("Cursor isn't signed in")?;
    let err = |e: String| format!("Couldn't get Cursor's quota: {e}");
    let period = post("https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage", token).map_err(err)?;
    let plan_info = post("https://api2.cursor.sh/aiserver.v1.DashboardService/GetPlanInfo", token).map_err(err)?;
    let info = [&plan_info["planInfo"], &plan_info["plan_info"], &plan_info].into_iter().find(|v| v.is_object()).cloned().unwrap_or(Value::Null);
    let plan = info["planName"].as_str().or(info["plan_name"].as_str()).filter(|s| !s.is_empty()).map(String::from);
    let usage = if period["planUsage"].is_object() { &period["planUsage"] } else { &period["plan_usage"] };
    let resets_at = time::stamp(&info["includedUsageResetsAt"]).or_else(|| time::stamp(&period["billingCycleEnd"])).map(|t| t as u64);
    let window = |id: &str, label: &str, used: &Value| {
        used.as_f64().or_else(|| used.as_str().and_then(|s| s.parse().ok())).map(|u| QuotaWindow {
            id: id.into(),
            label: label.into(),
            used_percent: u.clamp(0.0, 100.0),
            resets_at,
            ..Default::default()
        })
    };
    let period_label = match info["includedUsagePeriod"].as_str() {
        Some(p) if p.to_lowercase().contains("week") => "Week",
        Some(p) if p.to_lowercase().contains("month") => "Month",
        _ => "Included",
    };
    let mut windows = vec![];
    let total = window("cursor-included", period_label, &usage["totalPercentUsed"]);
    let auto = window("cursor-auto", "Auto", &usage["autoPercentUsed"]);
    if let Some(t) = &total {
        windows.push(t.clone());
    }
    if let Some(a) = auto.filter(|a| total.as_ref().is_none_or(|t| (t.used_percent - a.used_percent).abs() >= 0.5)) {
        windows.push(a);
    }
    windows.extend(window("cursor-api", "Cursor API", &usage["apiPercentUsed"]));
    let spend = if period["spendLimitUsage"].is_object() { &period["spendLimitUsage"] } else { &period["spend_limit_usage"] };
    for prefix in ["individual", "pooled"] {
        let (Some(used), Some(limit)) = (spend[format!("{prefix}Used")].as_f64(), spend[format!("{prefix}Limit")].as_f64()) else { continue };
        if limit > 0.0 {
            windows.push(QuotaWindow { id: "cursor-on-demand".into(), label: "On-demand".into(), used_percent: (used / limit * 100.0).clamp(0.0, 100.0), resets_at, ..Default::default() });
            break;
        }
    }
    Ok((plan, windows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_the_typed_prompt() {
        assert_eq!(prompt_text(&json!("<timestamp>x</timestamp>\n<user_query>\nfix it\n</user_query>")), "fix it");
        assert_eq!(prompt_text(&json!([{ "type": "text", "text": "<context>a</context> hello" }])), "hello");
        assert_eq!(model_name("default"), "Auto");
    }
}
