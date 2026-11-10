//! Grok Build: sessions under `~/.grok/sessions/<folder>/<id>` (`GROK_HOME` overrides
//! the folder): `summary.json` for each session, `updates.jsonl` for its turns. The
//! quota comes from Grok's billing endpoint with the sign-in in `auth.json`, read
//! only: an expired sign-in is reported, never renewed or rewritten here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nyx_agents_proto::{QuotaWindow, Usage, UsageSession};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logcache::LogCache;
use super::{Tally, json_line, quota, text, time};

const VERSION: u32 = 1;

pub fn base() -> PathBuf {
    std::env::var_os("GROK_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".grok"))
}

/// What one session's `updates.jsonl` adds up to. `turns`: by model and day,
/// (prompts, steps, input, output, cached). A turn is one prompt; its steps are the
/// model's replies, one per tool call plus the answer. Tool calls carry no time, so
/// they wait in `pending` for the turn that ends them.
#[derive(Default, Serialize, Deserialize)]
struct Updates {
    turns: HashMap<String, [u64; 5]>,
    tools: HashMap<String, u64>,
    pending: u64,
}

fn read_line(e: &mut Updates, line: &[u8], mtime: f64) {
    let Some(row) = json_line(line) else { return };
    let u = &row["params"]["update"];
    match u["sessionUpdate"].as_str() {
        Some("tool_call") => {
            e.pending += 1;
            let title = u["title"].as_str().unwrap_or("tool");
            let name = crate::sanitize::line(title.split(':').next().unwrap_or("tool"), 50);
            *e.tools.entry(name).or_default() += 1;
        }
        Some("turn_completed") => {
            let usage = &u["usage"];
            let at = time::stamp(&row["_meta"]["agentTimestampMs"]).or_else(|| time::stamp(&row["timestamp"])).unwrap_or(mtime);
            let day = time::local_day(at);
            let model = usage["modelUsage"].as_object().and_then(|m| m.keys().next().cloned()).unwrap_or_else(|| "grok".into());
            let model = model.trim_end_matches('/').rsplit('/').next().unwrap_or("grok").to_string();
            let n = |k: &str| usage[k].as_u64().unwrap_or(0);
            let c = e.turns.entry(format!("{}\x1f{day}", crate::sanitize::line(&model, 60))).or_default();
            c[0] += 1;
            c[1] += e.pending + 1;
            c[2] += n("inputTokens");
            c[3] += n("outputTokens");
            c[4] += if usage["cachedReadTokens"].is_null() { n("cacheReadInputTokens") } else { n("cachedReadTokens") };
            e.pending = 0;
        }
        _ => {}
    }
}

/// `%2Fhome%2Fken` → `/home/ken`.
fn unquote(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn session_dirs(base: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    let Ok(folders) = std::fs::read_dir(base.join("sessions")) else { return out };
    for f in folders.flatten().filter(|f| f.file_type().is_ok_and(|t| t.is_dir())) {
        let Ok(ids) = std::fs::read_dir(f.path()) else { continue };
        out.extend(ids.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()));
    }
    out.sort();
    out
}

/// Running grok programs (`grok` or `grok-<version>`): the session ids their command
/// lines name, and whether any runs at all.
fn running(base: &Path) -> (Vec<String>, bool) {
    let mut ids: Vec<String> = std::fs::read(base.join("active_sessions.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|i| i.as_str().or(i["id"].as_str()).map(String::from))
        .collect();
    let is_grok = |n: &str| n == "grok" || n.strip_prefix("grok-").is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'));
    let procs = super::procs::matching(is_grok);
    for p in &procs {
        ids.extend(p.argv.iter().filter_map(|a| super::codex::find_uuid(a)));
    }
    (ids, !procs.is_empty())
}

pub fn scan(base: &Path, refresh: bool) -> (Usage, Vec<UsageSession>) {
    let mut u = Usage { harness: "grok".into(), name: "Grok Build".into(), ..Default::default() };
    let mut t = Tally::default();
    let mut cache = LogCache::new("grok", VERSION);
    let mut sessions = vec![];
    for dir in session_dirs(base) {
        if let Some(rec) = cache.update(&dir.join("updates.jsonl"), read_line) {
            for (key, c) in &rec.entry.turns {
                let Some((model, day)) = key.split_once('\x1f') else { continue };
                t.prompt(model, day, c[0]);
                t.step(model, day, c[1]);
                t.tokens(model, day, c[2], c[3], c[4], 0);
            }
            for (name, n) in &rec.entry.tools {
                t.tool(name, *n);
            }
        }
        let Some(s) = std::fs::read(dir.join("summary.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let id = text(&s["info"]["id"], 100);
        if id.is_empty() {
            continue;
        }
        let cwd = s["info"]["cwd"].as_str().map(String::from).unwrap_or_else(|| {
            dir.parent().and_then(|p| p.file_name()).map(|n| unquote(&n.to_string_lossy())).unwrap_or_default()
        });
        let updated = s["updated_at"].as_str().or(s["last_active_at"].as_str()).and_then(time::parse_iso).unwrap_or(0.0) as u64;
        let folder = Path::new(&cwd).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "Work".into());
        let summary = text(&s["session_summary"], 150);
        sessions.push(UsageSession {
            id,
            title: if summary.is_empty() { format!("Session in {folder}") } else { summary },
            cwd,
            updated,
            prompts: s["num_chat_messages"].as_u64().or(s["num_messages"].as_u64()).unwrap_or(0),
            model: Some(text(&s["current_model_id"], 80)).filter(|m| !m.is_empty()),
            tokens: 0,
            active: false,
        });
    }
    cache.prune();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
    let (ids, any) = running(base);
    for s in sessions.iter_mut().filter(|s| ids.contains(&s.id)) {
        s.active = true;
    }
    // A grok that names no session runs the newest one.
    if any
        && ids.is_empty()
        && let Some(s) = sessions.first_mut()
    {
        s.active = true;
    }
    u.working = any;
    u.has_records = !sessions.is_empty();
    u.last_used = sessions.first().map(|s| s.updated);
    u.model = sessions.iter().find_map(|s| s.model.clone());
    u.today.sessions = sessions.iter().filter(|s| t.is_today(&time::local_day(s.updated as f64))).count() as u64;
    u.totals.sessions = sessions.len() as u64;
    t.finish(&mut u);
    sessions.truncate(super::HISTORY);
    u.sessions = sessions.iter().take(10).cloned().collect();
    u.quota = quota::get("grok", refresh, 180.0, || fetch_quota(base));
    (u, sessions)
}

/// The weekly credit pool and each product's share, from Grok's billing endpoint.
fn fetch_quota(base: &Path) -> quota::Fetched {
    let auth: Value = std::fs::read(base.join("auth.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).ok_or("Grok isn't signed in")?;
    let entry = auth.as_object().and_then(|o| o.values().find(|e| e["key"].as_str().is_some_and(|k| !k.trim().is_empty()))).ok_or("Grok isn't signed in")?;
    if entry["expires_at"].as_str().and_then(time::parse_iso).is_some_and(|t| t <= time::now()) {
        // Renewing it would mean rewriting Grok's own sign-in file; Grok does that itself.
        return Err("Grok's sign-in has expired; starting Grok renews it".into());
    }
    let token = entry["key"].as_str().unwrap_or_default().trim();
    let auth = format!("Bearer {token}");
    let data = quota::get_json("https://cli-chat-proxy.grok.com/v1/billing?format=credits", &[("Authorization", auth.as_str()), ("Accept", "application/json")])
        .map_err(|e| format!("Couldn't get Grok's quota: {e}"))?;
    let cfg = if data["config"].is_object() { &data["config"] } else { &data };
    let plan = cfg["subscriptionTier"].as_str().or(data["subscriptionTier"].as_str()).map(String::from);
    let resets_at = cfg["currentPeriod"]["end"].as_str().or(cfg["billingPeriodEnd"].as_str()).and_then(time::parse_iso).map(|t| t as u64);
    let mut windows = vec![QuotaWindow {
        id: "grok-weekly".into(),
        label: "Week".into(),
        used_percent: cfg["creditUsagePercent"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0),
        resets_at,
        ..Default::default()
    }];
    for p in cfg["productUsage"].as_array().into_iter().flatten() {
        let name = crate::sanitize::line(p["product"].as_str().unwrap_or("Product"), 40);
        windows.push(QuotaWindow {
            id: format!("grok-{}", name.to_lowercase().replace(' ', "-")),
            label: name,
            used_percent: p["usagePercent"].as_f64().unwrap_or(0.0).clamp(0.0, 100.0),
            resets_at,
            ..Default::default()
        });
    }
    Ok((plan, windows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_folder_names() {
        assert_eq!(unquote("%2Fhome%2Fken%2FProjects"), "/home/ken/Projects");
    }
}
