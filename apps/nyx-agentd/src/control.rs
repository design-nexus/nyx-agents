//! Typed actions on the session bus (`io.github.design_nexus.nyx.Control`),
//! found by Nyx AI's action registry like every Nyx app's. The manager
//! answers them, so they work while the app is closed.

use std::sync::Arc;

use nyx_agents_proto::Call;
use serde_json::{Value, json};

use crate::server::{self, Daemon};

pub const BUS_NAME: &str = "io.github.design_nexus.nyx.AgentManager";
const PATH: &str = "/io/github/design_nexus/nyx/AgentManager";

fn tools() -> Value {
    let read = json!({ "readOnlyHint": true });
    let id = json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] });
    json!([
        {"name": "agents.list_harnesses", "description": "Coding-agent harnesses Nyx Agents knows: installed or not, version, integration tier and supported capabilities.", "inputSchema": {"type": "object", "properties": {}}, "annotations": read},
        {"name": "agents.list_sessions", "description": "Agent sessions: harness, title, folder, branch, whether running, what the agent reports doing (working, blocked, done), turn count.", "inputSchema": {"type": "object", "properties": {}}, "annotations": read},
        {"name": "agents.session_status", "description": "One agent session in full, including its status message and subagents.", "inputSchema": id, "annotations": read},
        {"name": "agents.list_workspaces", "description": "Agent workspaces (groups of sessions shown together).", "inputSchema": {"type": "object", "properties": {}}, "annotations": read},
        {"name": "agents.open_session", "description": "Show an agent session in the Nyx Agents window (optionally one of its subagents).", "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}, "subagent": {"type": "string"}}, "required": ["id"]}, "annotations": {"readOnlyHint": false, "destructiveHint": false}},
    ])
}

struct Control {
    d: Arc<Daemon>,
}

#[zbus::interface(name = "io.github.design_nexus.nyx.Control")]
impl Control {
    fn describe(&self) -> String {
        tools().to_string()
    }

    fn call(&self, name: &str, arguments: &str) -> String {
        let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
        let result = self.dispatch(name, &args);
        match result {
            // Titles and status messages come from the agents themselves.
            Ok(v) => json!({ "ok": { "result": v, "untrusted_content": true } }),
            Err(e) => json!({ "error": e }),
        }
        .to_string()
    }
}

impl Control {
    fn dispatch(&self, name: &str, args: &Value) -> Result<Value, String> {
        let id = || args["id"].as_str().map(String::from).ok_or_else(|| "id is required".to_string());
        let call = match name {
            "agents.list_harnesses" => Call::ListHarnesses { refresh: false },
            "agents.list_sessions" => Call::ListSessions,
            "agents.session_status" => Call::GetSession { id: id()? },
            "agents.list_workspaces" => Call::ListWorkspaces,
            "agents.open_session" => {
                let id = id()?;
                if !nyx_agents_core::store::valid_id(&id) {
                    return Err("not a session id".into());
                }
                let mut target = format!("session/{id}");
                if let Some(sub) = args["subagent"].as_str() {
                    if !sub.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                        return Err("not a subagent id".into());
                    }
                    target.push_str(&format!("/subagent/{sub}"));
                }
                std::process::Command::new("nyx-agents")
                    .args(["--open", &target])
                    .stdin(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("cannot open Nyx Agents: {e}"))?;
                return Ok(json!({ "opened": target }));
            }
            other => return Err(format!("unknown action {other}")),
        };
        server::handle(&self.d, call).map_err(|e| e.message)
    }
}

/// Own the bus name and serve the interface. Without a session bus (a
/// headless login) the manager runs without it.
pub fn serve(d: Arc<Daemon>) -> Option<zbus::blocking::Connection> {
    let conn = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.name(BUS_NAME))
        .and_then(|b| b.serve_at(PATH, Control { d: d.clone() }))
        .and_then(|b| b.build());
    match conn {
        Ok(c) => Some(c),
        Err(e) => {
            d.log(&format!("session bus: {e}"));
            None
        }
    }
}
