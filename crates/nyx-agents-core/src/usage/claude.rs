//! Claude Code: transcripts under `~/.claude/projects` (`CLAUDE_CONFIG_DIR`
//! overrides), and the quota from Anthropic's OAuth usage endpoint.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use nyx_agents_proto::{QuotaWindow, Usage, UsageSession};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::logcache::LogCache;
use super::{Tally, json_line, quota, text, time};

/// Bump when `read_line` changes what it records, so caches are rebuilt.
const VERSION: u32 = 1;

pub fn base() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".claude"))
}

/// What one transcript adds up to, line by line.
///
/// `msgs`: one record per API message (model, day, step, input, output, cache read,
/// cache write). A message is written as several rows, one per content block, each
/// repeating its usage, so only its first row counts. `loose`: the same counts for
/// rows without a message id, by model and day. `prompt_log`: prompts credited to a
/// model, by row id; `pending`: prompts still waiting for the turn that names their
/// model.
#[derive(Default, Serialize, Deserialize)]
struct Transcript {
    model: String,
    cwd: String,
    preview: String,
    prompts: u64,
    tokens: u64,
    pending: Vec<(String, String)>,
    prompt_log: HashMap<String, (String, String)>,
    msgs: HashMap<String, (String, String, [u64; 5])>,
    loose: HashMap<String, [u64; 5]>,
    tools: HashMap<String, u64>,
}

/// A friendly model name, or None for placeholders.
pub fn model_name(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name == "<synthetic>" || name.eq_ignore_ascii_case("claude") {
        return None;
    }
    const KNOWN: &[(&str, &str)] = &[
        ("claude-opus-5-5", "Claude Opus 5.5"),
        ("claude-opus-5", "Claude Opus 5"),
        ("claude-opus-4-8", "Claude Opus 4.8"),
        ("claude-opus-4-6", "Claude Opus 4.6"),
        ("claude-opus-4-5", "Claude Opus 4.5"),
        ("claude-opus-4-1", "Claude Opus 4.1"),
        ("claude-opus-4", "Claude Opus 4"),
        ("claude-sonnet-5", "Claude Sonnet 5"),
        ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
        ("claude-sonnet-4-5", "Claude Sonnet 4.5"),
        ("claude-sonnet-4", "Claude Sonnet 4"),
        ("claude-haiku-4-5", "Claude Haiku 4.5"),
        ("claude-3-7-sonnet", "Claude 3.7 Sonnet"),
        ("claude-3-5-sonnet", "Claude 3.5 Sonnet"),
        ("claude-3-5-haiku", "Claude 3.5 Haiku"),
        ("claude-sonnet", "Claude Sonnet"),
        ("claude-opus", "Claude Opus"),
        ("claude-haiku", "Claude Haiku"),
    ];
    if let Some((_, nice)) = KNOWN.iter().find(|(k, _)| name.starts_with(k)) {
        return Some(nice.to_string());
    }
    let rest = name.strip_prefix("claude-").unwrap_or(name);
    let words: Vec<String> = rest
        .split('-')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect();
    Some(crate::sanitize::line(&format!("Claude {}", words.join(" ")), 60))
}

fn tag<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = s.find(&open)? + open.len();
    let end = s[start..].find(&close)? + start;
    Some(&s[start..end])
}

/// The text of a real prompt, or None for tool results and the CLI's own rows.
/// Slash commands count only with arguments; their text is the command as typed.
pub fn prompt_text(content: &Value) -> Option<String> {
    let joined;
    let s = match content {
        Value::String(s) => s.as_str(),
        Value::Array(blocks) => {
            let parts: Vec<&str> = blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
            if parts.is_empty() {
                return None;
            }
            joined = parts.join(" ");
            joined.as_str()
        }
        _ => return None,
    };
    let t = s.trim_start();
    if t.starts_with("<local-command-") {
        return None;
    }
    if t.starts_with("<command-") {
        let args = tag(s, "command-args").unwrap_or("").trim();
        if args.is_empty() {
            return None;
        }
        return Some(crate::sanitize::line(&format!("{} {args}", tag(s, "command-name").unwrap_or("").trim()), 140));
    }
    let line = crate::sanitize::line(s, 140);
    (!line.is_empty()).then_some(line)
}

fn read_line(e: &mut Transcript, line: &[u8], mtime: f64, subagent: bool, stem: &str) {
    let Some(row) = json_line(line) else { return };
    if !row.is_object() {
        return;
    }
    let message = if row["message"].is_object() { &row["message"] } else { &Value::Null };
    let role = message["role"].as_str().or(row["role"].as_str()).unwrap_or("");
    let day = time::local_day(time::stamp(&row["timestamp"]).unwrap_or(mtime));
    if e.cwd.is_empty()
        && let Some(c) = row["cwd"].as_str()
    {
        e.cwd = c.to_string();
    }
    if role == "user" && row["isMeta"] != true && !subagent {
        let content = if message["content"].is_null() { &row["content"] } else { &message["content"] };
        if let Some(p) = prompt_text(content) {
            e.prompts += 1;
            let uid = row["uuid"].as_str().map(String::from).unwrap_or_else(|| format!("{stem}:{}", e.prompts));
            e.pending.push((uid, day.clone()));
            if e.preview.is_empty() {
                e.preview = p;
            }
        }
    }
    let seen = message["model"].as_str().or(row["model"].as_str()).unwrap_or("");
    if let Some(m) = model_name(seen) {
        e.model = m;
    }
    if let Some(blocks) = message["content"].as_array() {
        for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
            *e.tools.entry(text(&b["name"], 60)).or_default() += 1;
        }
    }
    let step = u64::from(role == "assistant" && !e.model.is_empty() && seen != "<synthetic>");
    if step == 1 {
        for (uid, d) in e.pending.drain(..) {
            e.prompt_log.insert(uid, (e.model.clone(), d));
        }
    }
    let key = format!("{}\x1f{}", message["id"].as_str().unwrap_or(""), row["requestId"].as_str().unwrap_or(""));
    let keyed = key != "\x1f";
    if keyed && e.msgs.contains_key(&key) {
        return;
    }
    let usage = if message["usage"].is_object() { &message["usage"] } else { &row["usage"] };
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    let counts = [step, n("input_tokens"), n("output_tokens"), n("cache_read_input_tokens"), n("cache_creation_input_tokens")];
    e.tokens += counts[1] + counts[2] + counts[4];
    if keyed {
        e.msgs.insert(key, (e.model.clone(), day, counts));
    } else if !e.model.is_empty() && counts.iter().any(|c| *c > 0) {
        let l = e.loose.entry(format!("{}\x1f{day}", e.model)).or_default();
        l.iter_mut().zip(counts).for_each(|(a, b)| *a += b);
    }
}

fn add_usage(t: &mut Tally, model: &str, day: &str, c: &[u64; 5]) {
    if model.is_empty() {
        return;
    }
    if c[0] > 0 {
        t.step(model, day, c[0]);
    }
    t.tokens(model, day, c[1], c[2], c[3], c[4]);
}

fn jsonl_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => jsonl_files(&p, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x == "jsonl") => out.push(p),
            _ => {}
        }
    }
}

/// Claude processes that are not a session: the browser extension's messaging
/// host, one-shot prints and MCP servers.
const NOT_A_SESSION: &[&str] = &["--chrome-native-host", "-p", "--print", "mcp"];

fn running_cwds() -> Vec<String> {
    super::procs::named("claude")
        .into_iter()
        .filter(|p| !p.argv.iter().skip(1).any(|a| NOT_A_SESSION.contains(&a.as_str())))
        .filter_map(|p| p.cwd)
        .collect()
}

pub fn scan(base: &Path, refresh: bool) -> Usage {
    let mut u = Usage { harness: "claude".into(), name: "Claude Code".into(), ..Default::default() };
    let mut t = Tally::default();
    // A resumed or forked session copies earlier messages into a new transcript;
    // each message and prompt counts once across all of them.
    let mut seen_msgs: HashSet<String> = HashSet::new();
    let mut seen_prompts: HashSet<String> = HashSet::new();
    let mut cache = LogCache::new("claude", VERSION);
    let mut files = vec![];
    jsonl_files(&base.join("projects"), &mut files);
    files.sort();
    let mut sessions = vec![];
    for path in &files {
        // Subagent transcripts add steps and tokens but no prompts, and aren't sessions.
        let subagent = path.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "subagents");
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(rec) = cache.update(path, |e: &mut Transcript, line, mtime| read_line(e, line, mtime, subagent, &stem)) else { continue };
        let e = rec.entry;
        for (key, (model, day, c)) in &e.msgs {
            if seen_msgs.insert(key.clone()) {
                add_usage(&mut t, model, day, c);
            }
        }
        for (key, c) in &e.loose {
            if let Some((model, day)) = key.split_once('\x1f') {
                add_usage(&mut t, model, day, c);
            }
        }
        let mut prompts: Vec<(String, (String, String))> = e.prompt_log.into_iter().collect();
        if !e.model.is_empty() {
            prompts.extend(e.pending.iter().map(|(uid, d)| (uid.clone(), (e.model.clone(), d.clone()))));
        }
        for (uid, (model, day)) in prompts {
            if seen_prompts.insert(uid) {
                t.prompt(&model, &day, 1);
            }
        }
        for (name, n) in &e.tools {
            t.tool(name, *n);
        }
        if !subagent && (e.prompts > 0 || e.tokens > 0) && !e.model.is_empty() {
            sessions.push(UsageSession {
                id: stem.clone(),
                title: if e.preview.is_empty() { "Claude session".into() } else { e.preview },
                cwd: e.cwd,
                updated: rec.mtime as u64,
                prompts: e.prompts,
                model: Some(e.model),
                tokens: e.tokens,
                active: false,
            });
        }
    }
    cache.prune();
    sessions.sort_by_key(|s| std::cmp::Reverse(s.updated));
    // A running claude owns the newest session in its folder.
    let cwds = running_cwds();
    for cwd in &cwds {
        if let Some(s) = sessions.iter_mut().find(|s| !s.active && &s.cwd == cwd) {
            s.active = true;
        }
    }
    u.working = !cwds.is_empty();
    u.has_records = !sessions.is_empty();
    u.last_used = sessions.first().map(|s| s.updated);
    u.model = sessions.first().and_then(|s| s.model.clone());
    u.today.sessions = sessions.iter().filter(|s| t.is_today(&time::local_day(s.updated as f64))).count() as u64;
    u.totals.sessions = sessions.len() as u64;
    t.finish(&mut u);
    sessions.truncate(10);
    u.sessions = sessions;
    u.quota = quota::get("claude", refresh, 60.0, || fetch_quota(base));
    u
}

/// The 5-hour, weekly and per-model weekly windows, with the OAuth token Claude
/// Code keeps in `.credentials.json`.
fn fetch_quota(base: &Path) -> quota::Fetched {
    let creds: Value = std::fs::read(base.join(".credentials.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("Claude Code isn't signed in")?;
    let oauth = &creds["claudeAiOauth"];
    let token = oauth["accessToken"].as_str().ok_or("Claude Code isn't signed in")?;
    if oauth["expiresAt"].as_f64().is_some_and(|ms| ms / 1000.0 < time::now()) {
        // Claude Code renews it when it next starts; asking with it would only fail.
        return Err("Claude's sign-in has expired; starting Claude Code renews it".into());
    }
    let plan = oauth["subscriptionType"].as_str().filter(|s| !s.is_empty()).map(|s| {
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    });
    let auth = format!("Bearer {token}");
    let data = quota::get_json(
        "https://api.anthropic.com/api/oauth/usage",
        &[("Authorization", auth.as_str()), ("Content-Type", "application/json"), ("User-Agent", "Claude-Code/2.1.283")],
    )
    .map_err(|e| format!("Couldn't get Claude's quota: {e}"))?;
    let windows = [("five_hour", "5 hours"), ("seven_day", "Week"), ("seven_day_sonnet", "Week, Sonnet"), ("seven_day_opus", "Week, Opus")]
        .iter()
        .filter_map(|(key, label)| {
            let w = &data[*key];
            Some(QuotaWindow {
                id: format!("claude-{key}"),
                label: label.to_string(),
                used_percent: w["utilization"].as_f64()?.clamp(0.0, 100.0),
                resets_at: w["resets_at"].as_str().and_then(time::parse_iso).map(|t| t as u64),
                ..Default::default()
            })
        })
        .collect();
    Ok((plan, windows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_models() {
        assert_eq!(model_name("claude-opus-5-5-20261001").as_deref(), Some("Claude Opus 5.5"));
        assert_eq!(model_name("claude-mythos-2").as_deref(), Some("Claude Mythos 2"));
        assert_eq!(model_name("<synthetic>"), None);
    }

    #[test]
    fn finds_prompts() {
        assert_eq!(prompt_text(&json!("fix it")).as_deref(), Some("fix it"));
        assert_eq!(prompt_text(&json!([{ "type": "tool_result", "content": "x" }])), None);
        assert_eq!(prompt_text(&json!("<local-command-stdout>ok</local-command-stdout>")), None);
        assert_eq!(prompt_text(&json!("<command-name>/clear</command-name><command-args></command-args>")), None);
        assert_eq!(
            prompt_text(&json!("<command-name>/review</command-name><command-args>the parser</command-args>")).as_deref(),
            Some("/review the parser")
        );
    }
}
