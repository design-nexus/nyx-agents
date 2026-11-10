//! Agent workspaces: desktops of their own where agents open and drive GUI
//! programs, owned by the manager.
//!
//! A [`WorkspaceBackend`] makes the desktop; [`SwayWorkspaces`] runs each one
//! as a headless sway (`nyx-agent-workspace`). Workspaces end with the
//! manager, and one with no running sessions is removed after a grace period.
//! Sessions keep running whether or not anyone is looking at their workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nyx_agent_workspace::{self as ws, Capturer, Input, Instance, Options};
use nyx_agents_core::{new_id, paths, sanitize};
use nyx_agents_proto::{
    AgentWorkspace, Call, Event, InputAction, Lifecycle, RpcError, Screenshot, WorkspaceState, WorkspaceWindow, now_ms,
};
use serde_json::{Value, json};

use crate::server::Daemon;

/// How long a workspace with no running session is kept.
const GRACE: Duration = Duration::from_secs(10 * 60);

fn err(code: &str, message: &str) -> RpcError {
    RpcError { code: code.into(), message: message.into() }
}

/// One running desktop.
pub trait Desktop: Send + Sync {
    /// The opaque handle published as `AgentWorkspace.compositor`.
    fn handle(&self) -> String;
    fn alive(&self) -> bool;
    fn options(&self) -> Options;
    fn resize(&self, o: Options) -> Result<(), String>;
    fn launch(&self, argv: &[String], cwd: Option<&Path>) -> Result<u32, String>;
    fn windows(&self) -> Result<Vec<WorkspaceWindow>, String>;
    fn focus(&self, window: i64) -> Result<(), String>;
    fn screenshot(&self, max_w: u32, max_h: u32) -> Result<Screenshot, String>;
    /// Run input steps in order; the pointer's final position.
    fn input(&self, actions: &[InputAction]) -> Result<(u32, u32), String>;
    fn stop(&self);
}

/// Makes desktops. Sway today; a compositor-native backend could take its
/// place behind the same interface.
pub trait WorkspaceBackend: Send + Sync {
    fn create(&self, id: &str, o: Options) -> Result<Arc<dyn Desktop>, String>;
}

pub struct SwayWorkspaces;

impl WorkspaceBackend for SwayWorkspaces {
    fn create(&self, id: &str, o: Options) -> Result<Arc<dyn Desktop>, String> {
        let runtime = paths::runtime_dir().join("ws");
        let data = paths::state_dir().join("workspaces");
        paths::private_dir(&runtime).map_err(|e| e.to_string())?;
        paths::private_dir(&data).map_err(|e| e.to_string())?;
        let instance = Instance::start(id, &runtime, &data, o)?;
        Ok(Arc::new(SwayDesktop { instance, input: Mutex::new(None), capture: Mutex::new(None) }))
    }
}

struct SwayDesktop {
    instance: Instance,
    input: Mutex<Option<Input>>,
    capture: Mutex<Option<Capturer>>,
}

fn window(w: ws::Window) -> WorkspaceWindow {
    WorkspaceWindow {
        id: w.id,
        app_id: w.app_id.map(|a| sanitize::line(&a, 120)),
        title: sanitize::line(&w.title, 200),
        pid: w.pid,
        x: w.x,
        y: w.y,
        width: w.width,
        height: w.height,
        focused: w.focused,
        floating: w.floating,
    }
}

impl SwayDesktop {
    fn with_input<T>(&self, f: impl FnOnce(&mut Input) -> Result<T, String>) -> Result<T, String> {
        let mut slot = self.input.lock().unwrap();
        let (w, h) = self.instance.options().logical();
        if slot.is_none() {
            *slot = Some(Input::connect(&self.instance.wayland, w, h)?);
        }
        let input = slot.as_mut().unwrap();
        input.set_size(w, h);
        let r = f(input);
        if r.is_err() && !input.alive() {
            *slot = None;
        }
        r
    }
}

impl Desktop for SwayDesktop {
    fn handle(&self) -> String {
        format!("sway:{}", self.instance.wayland.display())
    }

    fn alive(&self) -> bool {
        self.instance.alive()
    }

    fn options(&self) -> Options {
        self.instance.options()
    }

    fn resize(&self, o: Options) -> Result<(), String> {
        self.instance.resize(o)
    }

    fn launch(&self, argv: &[String], cwd: Option<&Path>) -> Result<u32, String> {
        self.instance.launch(argv, cwd)
    }

    fn windows(&self) -> Result<Vec<WorkspaceWindow>, String> {
        Ok(self.instance.windows()?.into_iter().map(window).collect())
    }

    fn focus(&self, window: i64) -> Result<(), String> {
        self.instance.focus(window)
    }

    fn screenshot(&self, max_w: u32, max_h: u32) -> Result<Screenshot, String> {
        let mut slot = self.capture.lock().unwrap();
        if slot.is_none() {
            *slot = Some(Capturer::connect(&self.instance.wayland)?);
        }
        let frame = match slot.as_mut().unwrap().capture() {
            Ok(f) => f,
            Err(e) => {
                *slot = None;
                return Err(e);
            }
        };
        // Always at logical size or smaller, so screenshot pixels are pointer coordinates.
        let (lw, lh) = self.instance.options().logical();
        let frame = ws::shrink(&frame, lw.min(max_w), lh.min(max_h));
        let png = ws::png(&frame)?;
        Ok(Screenshot { width: frame.width, height: frame.height, png: ws::base64(&png) })
    }

    fn input(&self, actions: &[InputAction]) -> Result<(u32, u32), String> {
        self.with_input(|i| {
            for a in actions {
                i.apply(a)?;
            }
            Ok(i.position())
        })
    }

    fn stop(&self) {
        *self.input.lock().unwrap() = None;
        *self.capture.lock().unwrap() = None;
        self.instance.stop();
    }
}

struct Entry {
    record: AgentWorkspace,
    desktop: Arc<dyn Desktop>,
    used: Instant,
}

pub struct Workspaces {
    backend: Box<dyn WorkspaceBackend>,
    map: Mutex<HashMap<String, Entry>>,
}

impl Default for Workspaces {
    fn default() -> Self {
        Self { backend: Box::new(SwayWorkspaces), map: Mutex::new(HashMap::new()) }
    }
}

impl Workspaces {
    fn desktop(&self, id: &str) -> Result<Arc<dyn Desktop>, RpcError> {
        let mut map = self.map.lock().unwrap();
        let e = map.get_mut(id).ok_or_else(|| err("not-found", "no such workspace"))?;
        if !e.desktop.alive() {
            return Err(err("ended", "the workspace has ended"));
        }
        e.used = Instant::now();
        Ok(e.desktop.clone())
    }

    pub fn exists(&self, id: &str) -> bool {
        self.map.lock().unwrap().contains_key(id)
    }

    /// The Wayland socket of a running workspace.
    pub fn socket(&self, id: &str) -> Option<String> {
        let map = self.map.lock().unwrap();
        let e = map.get(id)?;
        e.record.wayland_socket().map(String::from)
    }
}

/// Workspaces end with the manager; remove what a previous run left.
pub fn clear_stale() {
    for dir in [paths::runtime_dir().join("ws"), paths::state_dir().join("workspaces")] {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn record(d: &Daemon, id: &str) -> Option<AgentWorkspace> {
    let mut w = d.workspaces.map.lock().unwrap().get(id)?.record.clone();
    let mut sessions: Vec<(u64, String)> = d
        .sessions
        .lock()
        .unwrap()
        .values()
        .filter(|s| s.workspace.as_deref() == Some(id))
        .map(|s| (s.created, s.id.clone()))
        .collect();
    sessions.sort();
    w.sessions = sessions.into_iter().map(|(_, s)| s).collect();
    Some(w)
}

/// Refresh one workspace's state and window summary; true if it changed.
fn refresh(d: &Daemon, id: &str) -> bool {
    let Some(desktop) = d.workspaces.map.lock().unwrap().get(id).map(|e| e.desktop.clone()) else { return false };
    let alive = desktop.alive();
    let windows = if alive { desktop.windows().ok() } else { None };
    let o = desktop.options();
    let mut map = d.workspaces.map.lock().unwrap();
    let Some(e) = map.get_mut(id) else { return false };
    let before = e.record.clone();
    e.record.state = if alive { WorkspaceState::Running } else { WorkspaceState::Ended };
    (e.record.width, e.record.height, e.record.scale) = (o.width, o.height, o.scale);
    if let Some(list) = windows {
        e.record.windows = list.len();
        e.record.focused = list.iter().find(|w| w.focused).map(|w| w.title.clone());
    } else if !alive {
        e.record.windows = 0;
        e.record.focused = None;
    }
    e.record != before
}

fn announce(d: &Daemon, id: &str) {
    if let Some(w) = record(d, id) {
        d.emit(&Event::Workspace { workspace: Box::new(w) });
    }
}

/// Start a workspace and publish it.
pub fn create(d: &Arc<Daemon>, name: Option<String>, o: Options) -> Result<AgentWorkspace, RpcError> {
    o.validate().map_err(|e| err("bad-size", &e))?;
    let id = new_id();
    let desktop = d.workspaces.backend.create(&id, o).map_err(|e| err("workspace", &e))?;
    let n = d.workspaces.map.lock().unwrap().len() + 1;
    let record = AgentWorkspace {
        id: id.clone(),
        name: name.map(|n| sanitize::line(&n, 80)).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Workspace {n}")),
        sessions: Vec::new(),
        compositor: Some(desktop.handle()),
        state: WorkspaceState::Running,
        width: o.width,
        height: o.height,
        scale: o.scale,
        created: now_ms(),
        windows: 0,
        focused: None,
    };
    d.log(&format!("workspace {id} started ({})", desktop.handle()));
    d.workspaces.map.lock().unwrap().insert(id.clone(), Entry { record, desktop, used: Instant::now() });
    announce(d, &id);
    record_or_err(d, &id)
}

fn record_or_err(d: &Daemon, id: &str) -> Result<AgentWorkspace, RpcError> {
    record(d, id).ok_or_else(|| err("not-found", "no such workspace"))
}

fn remove(d: &Daemon, id: &str) -> Result<(), RpcError> {
    let e = d.workspaces.map.lock().unwrap().remove(id).ok_or_else(|| err("not-found", "no such workspace"))?;
    e.desktop.stop();
    d.log(&format!("workspace {id} removed"));
    d.emit(&Event::WorkspaceRemoved { id: id.to_string() });
    Ok(())
}

pub fn handle(d: &Arc<Daemon>, call: Call) -> Result<Value, RpcError> {
    Ok(match call {
        Call::ListWorkspaces => {
            let ids: Vec<String> = d.workspaces.map.lock().unwrap().keys().cloned().collect();
            for id in &ids {
                refresh(d, id);
            }
            let mut all: Vec<AgentWorkspace> = ids.iter().filter_map(|id| record(d, id)).collect();
            all.sort_by_key(|w| w.created);
            json!(all)
        }
        Call::GetWorkspace { id } => {
            refresh(d, &id);
            json!(record_or_err(d, &id)?)
        }
        Call::CreateWorkspace { name, width, height, scale } => {
            let def = Options::default();
            let o = Options {
                width: width.unwrap_or(def.width),
                height: height.unwrap_or(def.height),
                scale: scale.unwrap_or(def.scale),
            };
            json!(create(d, name, o)?)
        }
        Call::RemoveWorkspace { id } => {
            remove(d, &id)?;
            json!({})
        }
        Call::ResizeWorkspace { id, width, height, scale } => {
            let desk = d.workspaces.desktop(&id)?;
            let o = Options { width, height, scale: scale.unwrap_or(desk.options().scale) };
            desk.resize(o).map_err(|e| err("workspace", &e))?;
            refresh(d, &id);
            announce(d, &id);
            json!(record_or_err(d, &id)?)
        }
        Call::LaunchInWorkspace { id, argv, cwd } => {
            if argv.is_empty() || argv.len() > 256 || argv.iter().any(|a| a.len() > 4096 || a.contains('\0')) {
                return Err(err("bad-argv", "argv must be 1 to 256 arguments"));
            }
            let cwd = cwd.map(PathBuf::from);
            if let Some(c) = &cwd
                && !c.is_dir()
            {
                return Err(err("bad-cwd", "working directory is not a folder"));
            }
            let pid = d.workspaces.desktop(&id)?.launch(&argv, cwd.as_deref()).map_err(|e| err("spawn", &e))?;
            d.log(&format!("workspace {id}: started {} (pid {pid})", argv[0]));
            json!({ "pid": pid })
        }
        Call::WorkspaceWindows { id } => json!(d.workspaces.desktop(&id)?.windows().map_err(|e| err("workspace", &e))?),
        Call::FocusWindow { id, window } => {
            d.workspaces.desktop(&id)?.focus(window).map_err(|e| err("workspace", &e))?;
            json!({})
        }
        Call::CaptureWorkspace { id, max_width, max_height } => {
            let shot = d.workspaces.desktop(&id)?.screenshot(max_width.unwrap_or(u32::MAX), max_height.unwrap_or(u32::MAX));
            json!(shot.map_err(|e| err("capture", &e))?)
        }
        Call::WorkspaceInput { id, actions } => {
            if actions.len() > 200 {
                return Err(err("too-many", "at most 200 input steps at once"));
            }
            let (x, y) = d.workspaces.desktop(&id)?.input(&actions).map_err(|e| err("input", &e))?;
            json!({ "x": x, "y": y })
        }
        _ => return Err(err("bad-request", "not a workspace call")),
    })
}

/// Every 2 seconds: publish window changes, and remove workspaces that have
/// ended or sat without a running session past the grace period.
pub fn supervise(d: Arc<Daemon>) {
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let ids: Vec<String> = d.workspaces.map.lock().unwrap().keys().cloned().collect();
        for id in ids {
            if refresh(&d, &id) {
                announce(&d, &id);
            }
            let live = d.sessions.lock().unwrap().values().any(|s| {
                s.workspace.as_deref() == Some(id.as_str()) && matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting)
            });
            let (ended, idle) = match d.workspaces.map.lock().unwrap().get(&id) {
                Some(e) => (e.record.state == WorkspaceState::Ended, e.used.elapsed() > GRACE),
                None => continue,
            };
            if !live && (ended || idle) {
                let _ = remove(&d, &id);
            }
        }
    }
}

/// Arguments that give a harness the computer-use tools for a workspace,
/// inserted right after the executable. Empty for harnesses not wired yet.
pub fn harness_args(harness: &str, workspace: &str, exe: &Path) -> Vec<String> {
    let exe = exe.display().to_string();
    let args = ["computer", "--workspace", workspace];
    match harness {
        "claude" => {
            let cfg = json!({ "mcpServers": { "nyx-computer": { "type": "stdio", "command": exe, "args": args } } });
            vec!["--mcp-config".into(), cfg.to_string()]
        }
        "codex" => vec![
            "-c".into(),
            format!("mcp_servers.nyx_computer.command={}", serde_json::to_string(&exe).unwrap()),
            "-c".into(),
            format!("mcp_servers.nyx_computer.args={}", serde_json::to_string(&args).unwrap()),
        ],
        _ => Vec::new(),
    }
}

/// The `nyx-agents` binary installed beside the manager.
pub fn computer_exe() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("nyx-agents")))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("nyx-agents"))
}

/// The environment a session in workspace `id` gets on top of the manager's.
pub fn session_env(d: &Daemon, id: &str) -> Vec<(String, String)> {
    let mut env = vec![("NYX_AGENT_WORKSPACE".to_string(), id.to_string())];
    if let Some(sock) = d.workspaces.socket(id) {
        env.push(("NYX_AGENT_WAYLAND_DISPLAY".into(), sock));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_gets_an_mcp_config() {
        let a = harness_args("claude", "w1", Path::new("/home/k/.local/bin/nyx-agents"));
        assert_eq!(a[0], "--mcp-config");
        let v: Value = serde_json::from_str(&a[1]).unwrap();
        let s = &v["mcpServers"]["nyx-computer"];
        assert_eq!(s["command"], "/home/k/.local/bin/nyx-agents");
        assert_eq!(s["args"], json!(["computer", "--workspace", "w1"]));
    }

    #[test]
    fn codex_gets_toml_overrides() {
        let a = harness_args("codex", "w1", Path::new("/opt/a \"b\"/nyx-agents"));
        assert_eq!(a[0], "-c");
        assert_eq!(a[1], r#"mcp_servers.nyx_computer.command="/opt/a \"b\"/nyx-agents""#);
        assert_eq!(a[3], r#"mcp_servers.nyx_computer.args=["computer","--workspace","w1"]"#);
        assert!(harness_args("grok", "w1", Path::new("x")).is_empty());
    }
}
