//! The Nyx Settings page (`nyx-agents settings pages|describe|set`), drawn
//! by Settings from the JSON printed here. Choices are kept in
//! `~/.config/nyx-agents/config.toml`, shared with the manager.

use nyx_agents_client::Client;
use nyx_agents_proto::{Activity, Harness, Lifecycle, Session};
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

/// The folder new sessions start in: the one chosen, else ~/Projects, else home.
pub fn default_folder() -> String {
    let home = nyx_agents_core::paths::home();
    get_str(&config(), "sessions", "default_folder")
        .filter(|p| std::path::Path::new(p).is_dir())
        .or_else(|| Some(home.join("Projects")).filter(|p| p.is_dir()).map(|p| p.display().to_string()))
        .unwrap_or_else(|| home.display().to_string())
}

pub fn set_default_folder(path: &str) -> std::io::Result<()> {
    set("sessions", "default_folder", toml::Value::String(path.into()))
}

pub fn notifications() -> bool {
    get_bool(&config(), "notifications", "enabled", true)
}

pub fn set_notifications(on: bool) -> std::io::Result<()> {
    set("notifications", "enabled", toml::Value::Boolean(on))
}

/// Agents whose provider reports a quota, how it is asked, and that in one short line.
const QUOTAS: &[(&str, &str, &str, &str)] = &[
    ("claude", "Claude Code", "Asks Anthropic with Claude Code's own sign-in.", "Asks Anthropic with Claude Code's own sign-in"),
    ("codex", "Codex", "Asks OpenAI through codex app-server, with no model calls.", "Asks OpenAI through codex app-server, with no model calls"),
    ("grok", "Grok Build", "Asks xAI with Grok's sign-in, read only: it is never renewed or rewritten here.", "Read only: its sign-in is never renewed here"),
    ("cursor", "Cursor", "Asks Cursor with its own sign-in.", "Asks Cursor with its own sign-in"),
    ("antigravity", "Antigravity", "Runs agy -p /usage.", "Runs agy -p /usage"),
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

/// The sessions the manager holds, when it answers.
fn sessions() -> Option<Vec<Session>> {
    Client::connect("nyx-agents-settings").and_then(|mut c| c.sessions()).ok()
}

/// Whether this Settings draws a preview pane beside the groups.
fn preview_supported() -> bool {
    std::env::var("SETTINGS_KINDS").is_ok_and(|k| k.split(' ').any(|x| x == "preview"))
}

/// The version number out of a `--version` line ("grok 1.0.77 (50ca…) [stable]" is "1.0.77").
fn short_version(v: &str) -> &str {
    v.split_whitespace()
        .map(|t| t.rsplit('/').next().unwrap_or(t).trim_start_matches('v'))
        .find(|t| t.starts_with(|c: char| c.is_ascii_digit()) && t.contains('.'))
        .unwrap_or(v)
}

/// What the harness can do beyond running in a terminal.
fn abilities(h: &Harness) -> Vec<&'static str> {
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
    can
}

fn page() -> Value {
    let cfg = config();
    let (list, running) = harnesses();
    if preview_supported() {
        return page_with_preview(&cfg, &list, running, if running { sessions() } else { None });
    }
    let installed: Vec<&Harness> = list.iter().filter(|h| h.path.is_some() && h.id != "shell").collect();
    let mut options: Vec<Value> = vec![json!(["", "The last one used"])];
    options.extend(installed.iter().map(|h| json!([h.id, h.name])));
    let rows: Vec<Value> = list
        .iter()
        .filter(|h| h.id != "shell")
        .map(|h| {
            let can = abilities(h);
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
        .map(|(id, name, how, _)| {
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
                  "value": get_str(&cfg, "sessions", "default_harness").unwrap_or_default(), "options": options },
                { "kind": "info", "key": "default_folder", "title": "Folder", "value": default_folder(),
                  "desc": "Where new sessions start. Change it in Agents' settings (Ctrl+,)." }
            ]},
            { "title": "Quotas", "note": "Shown in the AI Hub. Each is checked every few minutes while on; sign-ins are read by the session manager only and sent only to their provider.", "rows": quota_rows },
            { "title": "Agents on this computer", "note": "Found on your PATH and in mise's installs. Shims are never run.", "rows": rows },
            { "title": "Terminal", "rows": [
                { "kind": "info", "key": "font", "title": "Font", "value": "The system monospace font", "desc": "Change it under Appearance; colours follow the theme." }
            ]}
        ]
    })
}

/// The page as controls on the left and the agents on this computer on the right.
fn page_with_preview(cfg: &toml::Table, list: &[Harness], running: bool, sessions: Option<Vec<Session>>) -> Value {
    let agents: Vec<&Harness> = list.iter().filter(|h| h.id != "shell").collect();
    let installed: Vec<&Harness> = agents.iter().copied().filter(|h| h.path.is_some()).collect();
    let mut options: Vec<Value> = vec![json!(["", "The last one used"])];
    options.extend(installed.iter().map(|h| json!([h.id, h.name])));
    let on = quotas(cfg);
    let quota_rows: Vec<Value> = QUOTAS
        .iter()
        .map(|(id, name, _, short)| {
            json!({ "kind": "switch", "key": format!("quota-{id}"), "title": name, "desc": short,
                    "icon": "power-profile-performance-symbolic", "value": on.iter().any(|a| a == id) })
        })
        .collect();

    let mut stats = vec![if running {
        json!({ "label": "Session manager", "value": "Running", "sub": "Starts when Agents opens", "tone": "good" })
    } else {
        json!({ "label": "Session manager", "value": "Not running", "sub": "Starts when Agents opens" })
    }];
    if let Some(s) = sessions {
        let live = s.iter().filter(|s| matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)).count();
        let waiting =
            s.iter().filter(|s| s.lifecycle == Lifecycle::Running && s.status.activity == Activity::Blocked).count();
        let sub = match waiting {
            0 => "None waiting".to_string(),
            1 => "1 needs you".to_string(),
            n => format!("{n} need you"),
        };
        let mut item = json!({ "label": "Sessions", "value": format!("{live} open"), "sub": sub });
        if waiting > 0 {
            item["tone"] = json!("warn");
        }
        stats.push(item);
    }
    stats.push(json!({ "label": "Installed", "value": format!("{} of {}", installed.len(), agents.len()), "sub": "Agents to pick from" }));

    let found: Vec<Value> = agents
        .iter()
        .map(|h| {
            let can = abilities(h).join(" · ");
            let sub = match (&h.path, &h.version) {
                (Some(_), Some(v)) if can.is_empty() => short_version(v).to_string(),
                (Some(_), Some(v)) => format!("{} · {can}", short_version(v)),
                _ if can.is_empty() => "Runs in a terminal".to_string(),
                _ => can,
            };
            json!({ "icon": "utilities-terminal-symbolic", "label": h.name, "sub": sub,
                    "value": if h.path.is_some() { "Installed" } else { "Not installed" } })
        })
        .collect();

    json!({
        "poll": 5,
        "groups": [
            { "title": "Notifications", "note": "Agents that end unexpectedly are included; the session on screen is not notified about.", "rows": [
                { "kind": "switch", "key": "notifications", "title": "Notify when an agent needs you", "icon": "preferences-system-notifications-symbolic",
                  "desc": "Approvals, finished turns and errors", "value": get_bool(cfg, "notifications", "enabled", true) }
            ]},
            { "title": "New sessions", "note": "Change the folder in Agents' settings (Ctrl+,).", "rows": [
                { "kind": "choice", "key": "default_harness", "title": "Agent", "icon": "utilities-terminal-symbolic", "desc": "Listed first in New session",
                  "value": get_str(cfg, "sessions", "default_harness").unwrap_or_default(), "options": options },
                { "kind": "info", "key": "default_folder", "title": "Folder", "icon": "folder-symbolic", "value": default_folder(),
                  "desc": "Where new sessions start" }
            ]},
            { "title": "Quotas in the AI Hub", "note": "Each is checked every few minutes while on. Sign-ins are read by the session manager only and sent only to their own provider.", "rows": quota_rows },
            { "title": "Terminal", "rows": [
                { "kind": "info", "key": "font", "title": "Font", "icon": "preferences-desktop-font-symbolic", "value": "System monospace",
                  "desc": "Change it under Appearance; colours follow the theme" }
            ]}
        ],
        "preview": {
            "title": "Agents on this computer",
            "line": "Found on your PATH and in mise's installs. Shims are never run.",
            "blocks": [
                { "kind": "stats", "items": stats },
                { "kind": "list", "items": found }
            ],
            "foot": "Quotas turned on show in the AI Hub on the bar.",
            "foot_icon": "dialog-information-symbolic"
        }
    })
}

pub fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("pages") => {
            println!(
                "{}",
                json!([{ "id": "agents", "title": "Agents", "icon": "utilities-terminal-symbolic",
                         "description": "Coding agents: notifications, the default agent, what is installed",
                         "keywords": "claude codex agents ai notifications quota usage" }])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_versions() {
        assert_eq!(short_version("2.1.322 (Claude Code)"), "2.1.322");
        assert_eq!(short_version("codex-cli 0.167.2"), "0.167.2");
        assert_eq!(short_version("omp/18.6.1"), "18.6.1");
        assert_eq!(short_version("grok 1.0.77 (50ca579fa996) [stable]"), "1.0.77");
        assert_eq!(short_version("crush version v0.101.0"), "0.101.0");
        assert_eq!(short_version("nightly"), "nightly");
    }

    #[test]
    fn preview_page_has_icons_and_readings() {
        let p = page_with_preview(&toml::Table::new(), &[], false, None);
        for g in p["groups"].as_array().unwrap() {
            for r in g["rows"].as_array().unwrap() {
                assert!(r["icon"].is_string(), "{} has no icon", r["key"]);
            }
        }
        let blocks = p["preview"]["blocks"].as_array().unwrap();
        assert_eq!(blocks[0]["kind"], "stats");
        let labels: Vec<&str> = blocks[0]["items"].as_array().unwrap().iter().filter_map(|i| i["label"].as_str()).collect();
        assert_eq!(labels, ["Session manager", "Installed"], "no Sessions reading without the manager");
        assert_eq!(blocks[1]["kind"], "list");
    }
}
