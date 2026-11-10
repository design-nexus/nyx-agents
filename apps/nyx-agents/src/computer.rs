//! `nyx-agents computer [--workspace ID]`: computer-use tools for one agent
//! workspace, as an MCP server on stdin and stdout (newline-delimited
//! JSON-RPC). Harnesses start it for sessions that have a workspace.
//!
//! Without `--workspace` it finds the workspace of the session it runs under
//! (`NYX_AGENT_WORKSPACE` or `NYX_AGENTS_SESSION`, in its own environment or a
//! parent's), so one entry in a harness's global MCP config serves every
//! session. A session without a workspace gets no tools.
//!
//!   nyx-agents computer register [HARNESS...]   add that entry to harnesses
//!                                               that only read MCP servers from config
//!
//! It only talks to the manager, which owns the workspace; it never reaches
//! the user's desktop.

use std::io::{BufRead, Write};
use std::time::Duration;

use nyx_agents_client::Client;
use nyx_agents_proto::{AgentWorkspace, Call, InputAction, Screenshot, WorkspaceWindow};
use serde_json::{Value, json};

const PROTOCOL: &str = "2025-06-18";

fn instructions(w: Option<&AgentWorkspace>) -> String {
    let size = w.map(|w| format!("{}×{}", w.width, w.height)).unwrap_or_else(|| "1280×800".into());
    format!(
        "These tools drive a desktop of your own ({size}, sway with tabbed windows), separate from the user's screen. \
         The user can watch it live in Nyx Agents and may take over. Take a screenshot before acting; coordinates are \
         pixels in the latest screenshot. Start programs with open_app (an argv, no shell). Programs run as the user \
         and can read and change the user's files."
    )
}

fn tools() -> Value {
    let shot = json!({ "type": "boolean", "description": "Return a screenshot taken after the action." });
    let xy = |what: &str| {
        json!({
            "x": { "type": "integer", "description": format!("{what} x, in screenshot pixels") },
            "y": { "type": "integer", "description": format!("{what} y, in screenshot pixels") },
        })
    };
    let button = json!({ "type": "string", "enum": ["left", "right", "middle"] });
    let with = |mut props: Value, extra: Value| {
        for (k, v) in extra.as_object().unwrap() {
            props[k] = v.clone();
        }
        props
    };
    let act = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let read = json!({ "readOnlyHint": true, "openWorldHint": false });
    json!([
        { "name": "screenshot", "description": "A PNG of the whole workspace.",
          "inputSchema": { "type": "object", "properties": {} }, "annotations": read },
        { "name": "click", "description": "Click at x, y (or where the pointer is). count 2 double-clicks.",
          "inputSchema": { "type": "object", "properties": with(xy("Click"), json!({
              "button": button, "count": { "type": "integer", "minimum": 1, "maximum": 3 }, "screenshot": shot })) },
          "annotations": act },
        { "name": "move", "description": "Move the pointer (hover).",
          "inputSchema": { "type": "object", "properties": with(xy("Target"), json!({ "screenshot": shot })), "required": ["x", "y"] },
          "annotations": act },
        { "name": "drag", "description": "Press at from_x, from_y, move to to_x, to_y, release.",
          "inputSchema": { "type": "object", "properties": {
              "from_x": { "type": "integer" }, "from_y": { "type": "integer" },
              "to_x": { "type": "integer" }, "to_y": { "type": "integer" },
              "button": button, "screenshot": shot },
            "required": ["from_x", "from_y", "to_x", "to_y"] },
          "annotations": act },
        { "name": "scroll", "description": "Scroll by wheel clicks at x, y: positive dy is down, positive dx is right.",
          "inputSchema": { "type": "object", "properties": with(xy("Pointer"), json!({
              "dx": { "type": "integer" }, "dy": { "type": "integer" }, "screenshot": shot })) },
          "annotations": act },
        { "name": "type", "description": "Type text into the focused window.",
          "inputSchema": { "type": "object", "properties": { "text": { "type": "string" }, "screenshot": shot }, "required": ["text"] },
          "annotations": act },
        { "name": "key", "description": "Press a key or combo: Return, Escape, Tab, ctrl+l, ctrl+shift+t, alt+F4, Page_Down...",
          "inputSchema": { "type": "object", "properties": { "combo": { "type": "string" }, "screenshot": shot }, "required": ["combo"] },
          "annotations": act },
        { "name": "open_app", "description": "Start a program in the workspace from an argv, e.g. [\"firefox\", \"https://example.com\"] or [\"foot\"]. Browsers get a profile of their own.",
          "inputSchema": { "type": "object", "properties": {
              "argv": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
              "cwd": { "type": "string", "description": "Working directory (absolute)." },
              "screenshot": shot },
            "required": ["argv"] },
          "annotations": act },
        { "name": "list_windows", "description": "Windows in the workspace: id, app id, title, position and size, which is focused.",
          "inputSchema": { "type": "object", "properties": {} }, "annotations": read },
        { "name": "focus_window", "description": "Bring a window (by id from list_windows) to the front and focus it.",
          "inputSchema": { "type": "object", "properties": { "window": { "type": "integer" }, "screenshot": shot }, "required": ["window"] },
          "annotations": act },
        { "name": "resize_screen", "description": "Change the workspace size (640×480 to 3840×2160).",
          "inputSchema": { "type": "object", "properties": {
              "width": { "type": "integer" }, "height": { "type": "integer" } }, "required": ["width", "height"] },
          "annotations": act },
        { "name": "get_clipboard", "description": "The text on the workspace's clipboard (what a program there copied).",
          "inputSchema": { "type": "object", "properties": {} }, "annotations": read },
        { "name": "set_clipboard", "description": "Put text on the workspace's clipboard, for pasting into a program there.",
          "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] },
          "annotations": act },
    ])
}

struct Server {
    workspace: Option<String>,
    client: Option<Client>,
}

/// The workspace of the session this process runs under, from its own
/// environment or the nearest parent's that names one.
fn find_workspace(c: &mut Client) -> Option<String> {
    let mut pid = std::process::id();
    for _ in 0..32 {
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        let get = |key: &str| {
            env.split(|b| *b == 0)
                .find_map(|kv| kv.strip_prefix(key.as_bytes())?.strip_prefix(b"="))
                .map(|v| String::from_utf8_lossy(v).into_owned())
        };
        if let Some(w) = get("NYX_AGENT_WORKSPACE").filter(|w| !w.is_empty()) {
            return Some(w);
        }
        if let Some(s) = get("NYX_AGENTS_SESSION") {
            return c.session(&s).ok()?.workspace;
        }
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The parent pid follows the command name, which may itself hold spaces.
        let ppid: u32 = stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
        if ppid <= 1 {
            return None;
        }
        pid = ppid;
    }
    None
}

type ToolResult = Result<Vec<Value>, String>;

fn text(t: impl Into<String>) -> Value {
    json!({ "type": "text", "text": t.into() })
}

fn int(args: &Value, k: &str) -> Option<i32> {
    args[k].as_i64().map(|v| v.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
}

fn need(args: &Value, k: &str) -> Result<i32, String> {
    int(args, k).ok_or_else(|| format!("{k} is required"))
}

impl Server {
    fn ws(&self) -> Result<String, String> {
        self.workspace.clone().ok_or_else(|| "this session has no agent workspace".to_string())
    }

    fn call<T: serde::de::DeserializeOwned>(&mut self, call: Call) -> Result<T, String> {
        if self.client.is_none() {
            self.client = Some(Client::connect("nyx-agents-computer").map_err(|e| e.to_string())?);
        }
        let r = self.client.as_mut().unwrap().call(call);
        if matches!(r, Err(nyx_agents_client::Error::Io(_))) {
            self.client = None;
        }
        r.map_err(|e| e.to_string())
    }

    fn screenshot(&mut self) -> ToolResult {
        let s: Screenshot = self.call(Call::CaptureWorkspace { id: self.ws()?, max_width: None, max_height: None })?;
        Ok(vec![
            json!({ "type": "image", "data": s.png, "mimeType": "image/png" }),
            text(format!("{}×{} pixels", s.width, s.height)),
        ])
    }

    fn input(&mut self, actions: Vec<InputAction>) -> Result<String, String> {
        let r: Value = self.call(Call::WorkspaceInput { id: self.ws()?, actions })?;
        Ok(format!("Done; the pointer is at {}, {}.", r["x"], r["y"]))
    }

    fn tool(&mut self, name: &str, args: &Value) -> ToolResult {
        let button = args["button"].as_str().map(String::from);
        let mut out = match name {
            "screenshot" => return self.screenshot(),
            "click" => {
                let count = args["count"].as_u64().map(|c| c as u32);
                vec![text(self.input(vec![InputAction::Click { x: int(args, "x"), y: int(args, "y"), button, count }])?)]
            }
            "move" => vec![text(self.input(vec![InputAction::Move { x: need(args, "x")?, y: need(args, "y")? }])?)],
            "drag" => vec![text(self.input(vec![InputAction::Drag {
                from_x: need(args, "from_x")?,
                from_y: need(args, "from_y")?,
                to_x: need(args, "to_x")?,
                to_y: need(args, "to_y")?,
                button,
            }])?)],
            "scroll" => vec![text(self.input(vec![InputAction::Scroll {
                x: int(args, "x"),
                y: int(args, "y"),
                dx: int(args, "dx").unwrap_or(0),
                dy: int(args, "dy").unwrap_or(0),
            }])?)],
            "type" => {
                let t = args["text"].as_str().ok_or("text is required")?.to_string();
                vec![text(self.input(vec![InputAction::Type { text: t }])?)]
            }
            "key" => {
                let combo = args["combo"].as_str().ok_or("combo is required")?.to_string();
                vec![text(self.input(vec![InputAction::Key { combo }])?)]
            }
            "open_app" => {
                let argv: Vec<String> = args["argv"]
                    .as_array()
                    .ok_or("argv is required")?
                    .iter()
                    .map(|a| a.as_str().map(String::from).ok_or("argv must be strings"))
                    .collect::<Result<_, _>>()?;
                let cwd = args["cwd"].as_str().map(String::from);
                let r: Value = self.call(Call::LaunchInWorkspace { id: self.ws()?, argv, cwd })?;
                // Give the window a moment to appear before any screenshot.
                std::thread::sleep(Duration::from_millis(1500));
                vec![text(format!("Started (pid {}).", r["pid"]))]
            }
            "list_windows" => {
                let list: Vec<WorkspaceWindow> = self.call(Call::WorkspaceWindows { id: self.ws()? })?;
                return Ok(vec![text(serde_json::to_string_pretty(&list).unwrap())]);
            }
            "focus_window" => {
                let window = args["window"].as_i64().ok_or("window is required")?;
                self.call::<Value>(Call::FocusWindow { id: self.ws()?, window })?;
                vec![text("Focused.")]
            }
            "get_clipboard" => {
                let r: Value = self.call(Call::WorkspaceClipboardGet { id: self.ws()?, image: false })?;
                return Ok(vec![text(r["text"].as_str().unwrap_or("The clipboard is empty."))]);
            }
            "set_clipboard" => {
                let t = args["text"].as_str().ok_or("text is required")?.to_string();
                self.call::<Value>(Call::WorkspaceClipboardSet { id: self.ws()?, text: Some(t), png: None })?;
                vec![text("On the clipboard.")]
            }
            "resize_screen" => {
                let (width, height) = (need(args, "width")? as u32, need(args, "height")? as u32);
                let w: AgentWorkspace = self.call(Call::ResizeWorkspace { id: self.ws()?, width, height, scale: None })?;
                return Ok(vec![text(format!("The workspace is now {}×{}.", w.width, w.height))]);
            }
            other => return Err(format!("unknown tool {other}")),
        };
        if args["screenshot"].as_bool() == Some(true) {
            std::thread::sleep(Duration::from_millis(300));
            out.extend(self.screenshot()?);
        }
        Ok(out)
    }

    fn handle(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                if self.workspace.is_none()
                    && let Ok(mut c) = Client::connect("nyx-agents-computer")
                {
                    self.workspace = find_workspace(&mut c);
                }
                let w: Option<AgentWorkspace> = self.ws().ok().and_then(|id| self.call(Call::GetWorkspace { id }).ok());
                let version = params["protocolVersion"].as_str().unwrap_or(PROTOCOL);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "nyx-computer", "title": "Nyx agent workspace", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": instructions(w.as_ref()),
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": if self.workspace.is_some() { tools() } else { json!([]) } })),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default();
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                Ok(match self.tool(name, &args) {
                    Ok(content) => json!({ "content": content, "isError": false }),
                    Err(e) => json!({ "content": [text(e)], "isError": true }),
                })
            }
            _ => Err((-32601, format!("method not found: {method}"))),
        }
    }
}

pub fn run(args: &[String]) -> i32 {
    if args.first().is_some_and(|a| a == "register") {
        return crate::register::run(&args[1..]);
    }
    let workspace = match args.iter().position(|a| a == "--workspace").and_then(|i| args.get(i + 1)) {
        Some(w) if nyx_agents_core::store::valid_id(w) => Some(w.clone()),
        Some(_) => {
            eprintln!("usage: nyx-agents computer [--workspace ID]   (an MCP server on stdio)");
            return 2;
        }
        None => None,
    };
    let mut server = Server { workspace, client: None };
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => {
                let Some(id) = msg.get("id").cloned() else { continue };
                let method = msg["method"].as_str().unwrap_or_default();
                match server.handle(method, &msg["params"]) {
                    Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                    Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
                }
            }
            Err(e) => json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": e.to_string() } }),
        };
        if writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err() {
            break;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_have_object_schemas_and_unique_names() {
        let list = tools();
        let names: Vec<&str> = list.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        let mut dedup = names.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), names.len());
        for t in list.as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            for req in t["inputSchema"]["required"].as_array().into_iter().flatten() {
                assert!(t["inputSchema"]["properties"].get(req.as_str().unwrap()).is_some(), "{} requires {req}", t["name"]);
            }
        }
        assert!(names.contains(&"screenshot") && names.contains(&"open_app"));
    }

    #[test]
    fn unknown_methods_and_tools_are_errors() {
        let mut s = Server { workspace: Some("w".into()), client: None };
        assert_eq!(s.handle("nope", &json!({})).unwrap_err().0, -32601);
        let r = s.handle("tools/call", &json!({ "name": "teleport", "arguments": {} })).unwrap();
        assert_eq!(r["isError"], true);
        let r = s.handle("tools/call", &json!({ "name": "move", "arguments": {} })).unwrap();
        assert_eq!(r["content"][0]["text"], "x is required");
        assert!(s.handle("tools/list", &json!({})).unwrap()["tools"].is_array());
        let mut none = Server { workspace: None, client: None };
        assert_eq!(none.handle("tools/list", &json!({})).unwrap()["tools"], json!([]));
        let r = none.handle("tools/call", &json!({ "name": "screenshot", "arguments": {} })).unwrap();
        assert_eq!(r["content"][0]["text"], "this session has no agent workspace");
    }
}
