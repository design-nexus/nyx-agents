//! Glimpse plugin. It reads the session list Nyx Agents publishes (mounted
//! read-only by Glimpse as `$NYX_AGENTS_HUB`) and opens sessions through the
//! app's own `nyx-agents://` scheme. It never talks to the manager.

use anyhow::{Context, Result, bail};
use glimpse_plugin_sdk::{Action, Hit, serve};
use nyx_agents_proto::{Activity, Hub, HubSession};
use serde_json::{Value, json};

fn hub() -> Result<Hub> {
    let path = std::env::var_os("NYX_AGENTS_HUB").context("Nyx Agents isn't running.")?;
    let text = std::fs::read_to_string(path).context("Nyx Agents isn't running.")?;
    let hub: Hub = serde_json::from_str(&text).context("Nyx Agents' session list couldn't be read.")?;
    if hub.version != 1 {
        bail!("this Nyx Agents is newer than the plugin");
    }
    Ok(hub)
}

fn state(s: &HubSession) -> &'static str {
    match s.activity {
        Activity::Blocked => "Needs you",
        Activity::Done => "Done",
        Activity::Working => "Working",
        Activity::Error => "Error",
        Activity::Idle => "Idle",
        Activity::Unknown => "Running",
    }
}

fn haystack(s: &HubSession) -> String {
    format!("{} {} {} {} {}", s.title, s.harness, s.cwd, s.branch.as_deref().unwrap_or(""), state(s)).to_lowercase()
}

/// Sessions matching `query`. "agents" alone lists them all; "agents needs"
/// (or "waiting") lists those waiting for the user.
fn matches(hub: &Hub, query: &str) -> Vec<HubSession> {
    let q = query.trim().to_lowercase();
    let words: Vec<&str> = q.split_whitespace().collect();
    let (scoped, rest) = match words.split_first() {
        Some((first, rest)) if ["agent", "agents"].contains(first) => (true, rest.to_vec()),
        _ => (false, words.clone()),
    };
    if !scoped && rest.is_empty() {
        return Vec::new();
    }
    let needs = rest.iter().any(|w| ["needs", "waiting", "blocked"].contains(w));
    let mut out: Vec<HubSession> = hub
        .sessions
        .iter()
        .filter(|s| {
            if needs {
                return s.activity == Activity::Blocked;
            }
            let h = haystack(s);
            rest.iter().all(|w| h.contains(w))
        })
        .cloned()
        .collect();
    // Without the "agents" prefix, only clear title or folder matches count.
    if !scoped {
        out.retain(|s| rest.iter().any(|w| s.title.to_lowercase().contains(w) || s.cwd.to_lowercase().ends_with(w)));
    }
    out.sort_by_key(|s| (s.activity != Activity::Blocked, s.activity != Activity::Done, std::cmp::Reverse(s.status_since)));
    out
}

fn uri(s: &HubSession) -> Option<String> {
    s.id.chars().all(|c| c.is_ascii_hexdigit()).then(|| format!("nyx-agents://session/{}", s.id))
}

/// The sandbox has no view of the user's home, so folders show in full.
fn folder(cwd: &str) -> String {
    cwd.to_string()
}

fn hits(hub: &Hub, query: &str, limit: usize) -> Vec<Hit> {
    matches(hub, query)
        .into_iter()
        .filter_map(|s| {
            let url = uri(&s)?;
            let mut sub = vec![state(&s).to_string(), s.harness.clone()];
            if let Some(b) = &s.branch {
                sub.push(b.clone());
            }
            sub.push(folder(&s.cwd));
            if s.subagents_running > 0 {
                let n = s.subagents_running;
                sub.push(format!("{n} subagent{} running", if n == 1 { "" } else { "s" }));
            }
            Some(Hit { id: s.id.clone(), title: s.title.clone(), subtitle: sub.join(" · "), action: Action::Url { url } })
        })
        .take(limit)
        .collect()
}

fn preview(hub: &Hub, id: &str) -> Result<Value> {
    let s = hub.sessions.iter().find(|s| s.id == id).context("that session has ended")?;
    let mut lines = vec![s.title.clone(), format!("{} · {}", state(s), s.harness), format!("Folder: {}", folder(&s.cwd))];
    if let Some(b) = &s.branch {
        lines.push(format!("Branch: {b}"));
    }
    lines.push(format!("Turns: {}", s.turn));
    if s.subagents_total > 0 {
        lines.push(format!("Subagents: {} running of {}", s.subagents_running, s.subagents_total));
    }
    lines.push(String::new());
    lines.push("Enter opens it in Agents.".into());
    Ok(json!({ "text": lines.join("\n") }))
}

fn main() -> Result<()> {
    serve(&["search", "preview"], |req| match req.op.as_str() {
        "search" => {
            let limit = req.args["limit"].as_u64().unwrap_or(10).min(10) as usize;
            let q = req.args["query"].as_str().unwrap_or("");
            // No list (manager not running) just means no results.
            Ok(hub().map(|h| json!(hits(&h, q, limit))).unwrap_or_else(|_| json!([])))
        }
        "preview" => preview(&hub()?, req.args["result_id"].as_str().unwrap_or("")),
        other => bail!("unsupported operation {other}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str, activity: Activity) -> HubSession {
        HubSession {
            id: id.into(),
            harness: "claude".into(),
            title: title.into(),
            project: None,
            cwd: "/home/ken/Projects/nyx-agents".into(),
            branch: Some("main".into()),
            activity,
            blocked_on: None,
            turn: 2,
            started: 1,
            status_since: 2,
            subagents_running: 0,
            subagents_total: 0,
            workspace: None,
            open: vec![],
        }
    }

    fn hub() -> Hub {
        Hub {
            version: 1,
            sessions: vec![
                session("aa11", "Claude Code · nyx-agents", Activity::Working),
                session("bb22", "Fix the parser", Activity::Blocked),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn agents_lists_sessions_waiting_first() {
        let h = hits(&hub(), "agents", 10);
        assert_eq!(h.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["bb22", "aa11"]);
        assert!(matches!(&h[0].action, Action::Url { url } if url == "nyx-agents://session/bb22"));
        assert_eq!(hits(&hub(), "agents needs", 10).len(), 1);
    }

    #[test]
    fn plain_words_need_a_title_or_folder_match() {
        assert_eq!(hits(&hub(), "parser", 10).len(), 1);
        assert!(hits(&hub(), "main", 10).is_empty(), "a branch alone is too loose without 'agents'");
        assert_eq!(hits(&hub(), "agents main", 10).len(), 2);
        assert!(hits(&hub(), "", 10).is_empty());
    }

    #[test]
    fn never_builds_a_uri_from_an_odd_id() {
        let mut h = hub();
        h.sessions[0].id = "x/../y".into();
        assert!(hits(&h, "agents", 10).iter().all(|x| x.id != "x/../y"));
    }
}
