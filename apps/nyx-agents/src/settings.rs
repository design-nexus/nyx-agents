//! The Nyx Settings page (`nyx-agents settings pages|describe|set`), drawn
//! by Settings from the JSON printed here. Choices are kept in
//! `~/.config/nyx-agents/config.toml`, shared with the manager.

use nyx_agents_client::Client;
use nyx_agents_proto::Harness;
use serde_json::{Value, json};

fn config_path() -> std::path::PathBuf {
    nyx_agents_core::paths::config_dir().join("config.toml")
}

fn config() -> toml::Table {
    std::fs::read_to_string(config_path()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

fn get_bool(t: &toml::Table, section: &str, key: &str, default: bool) -> bool {
    t.get(section).and_then(|s| s.get(key)).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn get_str(t: &toml::Table, section: &str, key: &str) -> Option<String> {
    t.get(section).and_then(|s| s.get(key)).and_then(|v| v.as_str()).map(String::from)
}

fn set(section: &str, key: &str, value: toml::Value) -> std::io::Result<()> {
    let mut t = config();
    let s = t.entry(section.to_string()).or_insert_with(|| toml::Value::Table(Default::default()));
    if let Some(s) = s.as_table_mut() {
        s.insert(key.to_string(), value);
    }
    let dir = nyx_agents_core::paths::config_dir();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(config_path(), toml::to_string(&t).unwrap_or_default())
}

/// The harness chosen for new sessions, if one was.
pub fn default_harness() -> Option<String> {
    get_str(&config(), "sessions", "default_harness")
}

pub fn set_default_harness(id: &str) -> std::io::Result<()> {
    set("sessions", "default_harness", toml::Value::String(id.into()))
}

pub fn notifications() -> bool {
    get_bool(&config(), "notifications", "enabled", true)
}

pub fn set_notifications(on: bool) -> std::io::Result<()> {
    set("notifications", "enabled", toml::Value::Boolean(on))
}

/// Agents whose provider reports a quota, and how it is asked.
const QUOTAS: &[(&str, &str, &str)] = &[
    ("claude", "Claude Code", "Asks Anthropic with Claude Code's own sign-in."),
    ("codex", "Codex", "Asks OpenAI through codex app-server, with no model calls."),
    ("grok", "Grok Build", "Asks xAI with Grok's sign-in, read only: it is never renewed or rewritten here."),
    ("cursor", "Cursor", "Asks Cursor with its own sign-in."),
    ("antigravity", "Antigravity", "Runs agy -p /usage."),
];

/// The agents listed in `[usage] quotas`.
fn quotas(t: &toml::Table) -> Vec<String> {
    t.get("usage").and_then(|u| u.get("quotas")).and_then(|q| q.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn set_quota(agent: &str, on: bool) -> std::io::Result<()> {
    let mut list = quotas(&config());
    list.retain(|a| a != agent);
    if on {
        list.push(agent.to_string());
    }
    set("usage", "quotas", toml::Value::Array(list.into_iter().map(toml::Value::String).collect()))
}

fn harnesses() -> (Vec<Harness>, bool) {
    match Client::connect("nyx-agents-settings").and_then(|mut c| c.harnesses(false)) {
        Ok(h) => (h, true),
        Err(_) => (nyx_agents_core::adapters::harnesses(false), false),
    }
}

fn page() -> Value {
    let cfg = config();
    let (list, running) = harnesses();
    let installed: Vec<&Harness> = list.iter().filter(|h| h.path.is_some() && h.id != "shell").collect();
    let mut options: Vec<Value> = vec![json!(["", "The last one used"])];
    options.extend(installed.iter().map(|h| json!([h.id, h.name])));
    let rows: Vec<Value> = list
        .iter()
        .filter(|h| h.id != "shell")
        .map(|h| {
            let c = &h.capabilities;
            let mut can = Vec::new();
            if c.program_status {
                can.push("status");
            }
            if c.resume {
                can.push("resume");
            }
            if c.subagents.is_some() {
                can.push("subagents");
            }
            let value = match (&h.path, &h.version) {
                (None, _) => "Not installed".to_string(),
                (Some(_), Some(v)) => v.clone(),
                (Some(_), None) => "Installed".to_string(),
            };
            json!({
                "kind": "info", "key": format!("h-{}", h.id), "title": h.name, "value": value,
                "desc": if can.is_empty() { "Runs in a terminal".to_string() } else { format!("Runs in a terminal; {}", can.join(", ")) },
            })
        })
        .collect();
    let on = quotas(&cfg);
    let quota_rows: Vec<Value> = QUOTAS
        .iter()
        .map(|(id, name, how)| {
            json!({ "kind": "switch", "key": format!("quota-{id}"), "title": format!("Check {name}'s quota"), "desc": how, "value": on.iter().any(|a| a == id) })
        })
        .collect();
    json!({
        "subtitle": if running { "The session manager is running" } else { "The session manager starts when Agents opens" },
        "groups": [
            { "title": "Notifications", "rows": [
                { "kind": "switch", "key": "notifications", "title": "Notify when an agent needs you",
                  "desc": "Approval requests, finished turns, errors and agents that end unexpectedly. The session on screen is not notified about.",
                  "value": get_bool(&cfg, "notifications", "enabled", true) }
            ]},
            { "title": "New sessions", "rows": [
                { "kind": "choice", "key": "default_harness", "title": "Agent", "desc": "Listed first in New session.",
                  "value": get_str(&cfg, "sessions", "default_harness").unwrap_or_default(), "options": options }
            ]},
            { "title": "Quotas", "note": "Shown in the AI Hub. Each is checked every few minutes while on; sign-ins are read by the session manager only and sent only to their provider.", "rows": quota_rows },
            { "title": "Agents on this computer", "note": "Found on your PATH and in mise's installs. Shims are never run.", "rows": rows },
            { "title": "Terminal", "rows": [
                { "kind": "info", "key": "font", "title": "Font", "value": "The system monospace font", "desc": "Change it under Appearance; colours follow the theme." }
            ]}
        ]
    })
}

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("pages") => {
            println!(
                "{}",
                json!([{ "id": "agents", "title": "Agents", "icon": "utilities-terminal-symbolic",
                         "description": "Coding agents: notifications, the default agent, what is installed",
                         "keywords": ["claude", "codex", "agents", "ai", "notifications", "quota", "usage"] }])
            );
            0
        }
        Some("describe") => {
            println!("{}", page());
            0
        }
        Some("set") => {
            let (key, value) = (args.get(2).map(String::as_str), args.get(3).cloned().unwrap_or_default());
            let r = match key {
                Some("notifications") => set("notifications", "enabled", toml::Value::Boolean(value == "true")),
                Some(k) if k.strip_prefix("quota-").is_some_and(|a| QUOTAS.iter().any(|(id, ..)| *id == a)) => {
                    set_quota(&k["quota-".len()..], value == "true")
                }
                Some("default_harness") if value.is_empty() || value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') => {
                    set("sessions", "default_harness", toml::Value::String(value))
                }
                _ => {
                    eprintln!("unknown setting");
                    return 1;
                }
            };
            match r {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("could not save: {e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: nyx-agents settings pages|describe PAGE|set PAGE KEY VALUE");
            2
        }
    }
}
