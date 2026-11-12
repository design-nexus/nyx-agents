//! Agent workspaces: desktops of their own where agents open and drive GUI
//! programs, owned by the manager.
//!
//! A [`WorkspaceBackend`] makes the desktop; [`SwayWorkspaces`] runs each one
//! as a headless sway (`nyx-agent-workspace`) under a holder process of its
//! own (`ws_holder`), so workspaces outlive the manager and are picked up
//! again when it restarts. One with no running sessions is removed after a
//! grace period. Sessions keep running whether or not anyone is looking at
//! their workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nyx_agent_workspace::{self as ws, Capturer, Input, Options};
use nyx_agents_core::{new_id, sanitize};
use nyx_agents_proto::{
    AgentWorkspace, Call, Event, InputAction, Lifecycle, RpcError, Screenshot, WorkspaceState, WorkspaceWindow, now_ms,
};
use serde_json::{Value, json};

use crate::server::Daemon;
use crate::ws_holder::{self, HolderState, Request};

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
    /// The clipboard in the first of `types` it offers.
    fn clipboard_get(&self, types: &[String]) -> Result<Option<(String, Vec<u8>)>, String>;
    /// Offer `items` (mime type, bytes) as the clipboard.
    fn clipboard_set(&self, items: Vec<(String, Vec<u8>)>) -> Result<(), String>;
    /// The X display for X11 programs, if it has one.
    fn x_display(&self) -> Option<String>;
    fn stop(&self);
}

/// Makes desktops. Sway today; a compositor-native backend could take its
/// place behind the same interface.
pub trait WorkspaceBackend: Send + Sync {
    fn create(&self, id: &str, name: &str, o: Options) -> Result<Arc<dyn Desktop>, String>;
}

pub struct SwayWorkspaces;

impl WorkspaceBackend for SwayWorkspaces {
    fn create(&self, id: &str, name: &str, o: Options) -> Result<Arc<dyn Desktop>, String> {
        use std::io::BufRead;
        use std::os::unix::process::CommandExt;
        let mut cmd = crate::self_command();
        cmd.arg("ws-hold")
            .args(["--id", id, "--name", name])
            .args(["--width", &o.width.to_string(), "--height", &o.height.to_string(), "--scale", &o.scale.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        // Its own session, so it outlives the manager like a session's holder.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn().map_err(|e| format!("cannot start the workspace: {e}"))?;
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufReader::new(out).read_line(&mut line);
            let _ = tx.send(line);
        });
        let line = rx.recv_timeout(Duration::from_secs(30)).map_err(|_| "the workspace did not start in time".to_string())?;
        let v: Value = serde_json::from_str(&line).map_err(|_| "the workspace ended while starting".to_string())?;
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            return Err(e.to_string());
        }
        let st: HolderState = serde_json::from_value(v).map_err(|e| e.to_string())?;
        Ok(Arc::new(SwayDesktop::attach(st)))
    }
}

/// A workspace reached through its holder (launch, resize, clipboard, stop)
/// and its sockets (windows, input, capture).
struct SwayDesktop {
    dir: PathBuf,
    state: Mutex<HolderState>,
    input: Mutex<Option<Input>>,
    capture: Mutex<Option<Capturer>>,
}

impl SwayDesktop {
    fn attach(st: HolderState) -> Self {
        Self {
            dir: ws_holder::runtime_root().join(&st.id),
            state: Mutex::new(st),
            input: Mutex::new(None),
            capture: Mutex::new(None),
        }
    }

    fn wayland(&self) -> PathBuf {
        self.state.lock().unwrap().wayland.clone()
    }

    fn ipc(&self) -> Result<ws::Ipc, String> {
        let sock = self.state.lock().unwrap().swaysock.clone();
        ws::Ipc::connect(&sock).map_err(|e| format!("workspace not reachable: {e}"))
    }

    fn with_input<T>(&self, f: impl FnOnce(&mut Input) -> Result<T, String>) -> Result<T, String> {
        let mut slot = self.input.lock().unwrap();
        let (w, h) = self.options().logical();
        if slot.is_none() {
            *slot = Some(Input::connect(&self.wayland(), w, h)?);
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

impl Desktop for SwayDesktop {
    fn handle(&self) -> String {
        format!("sway:{}", self.wayland().display())
    }

    fn alive(&self) -> bool {
        ws_holder::alive(self.state.lock().unwrap().pid)
    }

    fn options(&self) -> Options {
        self.state.lock().unwrap().options
    }

    fn resize(&self, o: Options) -> Result<(), String> {
        o.validate()?;
        ws_holder::call(&self.dir, &Request::Resize { options: o })?;
        self.state.lock().unwrap().options = o;
        Ok(())
    }

    fn launch(&self, argv: &[String], cwd: Option<&Path>) -> Result<u32, String> {
        let v = ws_holder::call(&self.dir, &Request::Launch { argv: argv.to_vec(), cwd: cwd.map(Path::to_path_buf) })?;
        v["pid"].as_u64().map(|p| p as u32).ok_or_else(|| "the workspace gave no pid".into())
    }

    fn windows(&self) -> Result<Vec<WorkspaceWindow>, String> {
        let tree = self.ipc()?.tree().map_err(|e| e.to_string())?;
        Ok(ws::windows(&tree).into_iter().map(window).collect())
    }

    fn focus(&self, window: i64) -> Result<(), String> {
        self.ipc()?.run(&format!("[con_id={window}] focus"))
    }

    fn screenshot(&self, max_w: u32, max_h: u32) -> Result<Screenshot, String> {
        let mut slot = self.capture.lock().unwrap();
        if slot.is_none() {
            *slot = Some(Capturer::connect(&self.wayland())?);
        }
        let frame = match slot.as_mut().unwrap().capture() {
            Ok(f) => f,
            Err(e) => {
                *slot = None;
                return Err(e);
            }
        };
        // Always at logical size or smaller, so screenshot pixels are pointer coordinates.
        let (lw, lh) = self.options().logical();
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

    fn clipboard_get(&self, types: &[String]) -> Result<Option<(String, Vec<u8>)>, String> {
        let v = ws_holder::call(&self.dir, &Request::ClipboardGet { types: types.to_vec() })?;
        let Some(mime) = v["mime"].as_str() else { return Ok(None) };
        let data = v["data"].as_str().and_then(ws::unbase64).ok_or("the workspace sent a bad clipboard")?;
        Ok(Some((mime.to_string(), data)))
    }

    fn clipboard_set(&self, items: Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let items = items.into_iter().map(|(m, d)| (m, ws::base64(&d))).collect();
        ws_holder::call(&self.dir, &Request::ClipboardSet { items }).map(drop)
    }

    fn x_display(&self) -> Option<String> {
        self.state.lock().unwrap().display.clone()
    }

    fn stop(&self) {
        *self.input.lock().unwrap() = None;
        *self.capture.lock().unwrap() = None;
        let pid = self.state.lock().unwrap().pid;
        let _ = ws_holder::call(&self.dir, &Request::Stop);
        let start = Instant::now();
        while ws_holder::alive(pid) && start.elapsed() < Duration::from_secs(4) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if ws_holder::alive(pid) {
            // Its sway and programs end with it.
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
        }
        ws::remove_tree(&self.dir);
        let id = self.state.lock().unwrap().id.clone();
        ws::remove_tree(&ws_holder::data_root().join(id));
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

    pub fn x_display(&self, id: &str) -> Option<String> {
        self.map.lock().unwrap().get(id)?.desktop.x_display()
    }

    /// The Wayland socket of a running workspace.
    pub fn socket(&self, id: &str) -> Option<String> {
        let map = self.map.lock().unwrap();
        let e = map.get(id)?;
        e.record.wayland_socket().map(String::from)
    }
}

/// Pick up the workspaces whose holders are still running (the manager
/// restarted) and remove what ended ones left behind.
pub fn restore(d: &Daemon) {
    let mut live = Vec::new();
    for e in std::fs::read_dir(ws_holder::runtime_root()).into_iter().flatten().flatten() {
        let st = std::fs::read(ws_holder::state_file(&e.path()))
            .ok()
            .and_then(|b| serde_json::from_slice::<HolderState>(&b).ok())
            .filter(|st| ws_holder::alive(st.pid));
        match st {
            Some(st) => live.push(st),
            None => {
                ws::remove_tree(&e.path());
            }
        }
    }
    for e in std::fs::read_dir(ws_holder::data_root()).into_iter().flatten().flatten() {
        if !live.iter().any(|st| e.file_name() == st.id.as_str()) {
            ws::remove_tree(&e.path());
        }
    }
    for st in live {
        let o = st.options;
        let record = AgentWorkspace {
            id: st.id.clone(),
            name: st.name.clone(),
            sessions: Vec::new(),
            compositor: Some(format!("sway:{}", st.wayland.display())),
            state: WorkspaceState::Running,
            width: o.width,
            height: o.height,
            scale: o.scale,
            created: st.created,
            windows: 0,
            focused: None,
        };
        d.log(&format!("workspace {} picked up again", st.id));
        let id = st.id.clone();
        let desktop: Arc<dyn Desktop> = Arc::new(SwayDesktop::attach(st));
        d.workspaces.map.lock().unwrap().insert(id, Entry { record, desktop, used: Instant::now() });
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
    let n = d.workspaces.map.lock().unwrap().len() + 1;
    let name = name.map(|n| sanitize::line(&n, 80)).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Workspace {n}"));
    let desktop = d.workspaces.backend.create(&id, &name, o).map_err(|e| err("workspace", &e))?;
    let record = AgentWorkspace {
        id: id.clone(),
        name,
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
        Call::WorkspaceClipboardGet { id, image } => {
            let desk = d.workspaces.desktop(&id)?;
            if image {
                let got = desk.clipboard_get(&["image/png".to_string()]).map_err(|e| err("clipboard", &e))?;
                json!({ "png": got.map(|(_, b)| ws::base64(&b)) })
            } else {
                let types: Vec<String> = ws::TEXT_TYPES.iter().map(|t| t.to_string()).collect();
                let got = desk.clipboard_get(&types).map_err(|e| err("clipboard", &e))?;
                json!({ "text": got.map(|(_, b)| String::from_utf8_lossy(&b).into_owned()) })
            }
        }
        Call::WorkspaceClipboardSet { id, text, png } => {
            let desk = d.workspaces.desktop(&id)?;
            let items: Vec<(String, Vec<u8>)> = match (text, png) {
                (Some(t), None) => ws::TEXT_TYPES.iter().map(|m| (m.to_string(), t.clone().into_bytes())).collect(),
                (None, Some(p)) => {
                    let bytes = ws::unbase64(&p).ok_or_else(|| err("bad-png", "png is not base64"))?;
                    if !bytes.starts_with(b"\x89PNG") {
                        return Err(err("bad-png", "not a PNG image"));
                    }
                    vec![("image/png".to_string(), bytes)]
                }
                _ => return Err(err("bad-request", "give text or png")),
            };
            desk.clipboard_set(items).map_err(|e| err("clipboard", &e))?;
            json!({})
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
        "muse" => {
            let server = json!({ "command": exe, "args": args });
            vec!["-c".into(), format!("mcpServers.nyx-computer={server}")]
        }
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

/// The environment a session in workspace `id` gets on top of the manager's:
/// GUI programs the agent starts from its own shell open in its workspace.
pub fn session_env(d: &Daemon, id: &str) -> (Vec<(String, String)>, Vec<&'static str>) {
    let mut env = vec![("NYX_AGENT_WORKSPACE".to_string(), id.to_string())];
    let mut remove = vec!["DISPLAY"];
    if let Some(sock) = d.workspaces.socket(id) {
        env.push(("NYX_AGENT_WAYLAND_DISPLAY".into(), sock.clone()));
        env.push(("WAYLAND_DISPLAY".into(), sock));
    }
    if let Some(x) = d.workspaces.x_display(id) {
        remove.clear();
        env.push(("DISPLAY".into(), x));
    }
    (env, remove)
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
        let m = harness_args("muse", "w1", Path::new("/bin/nyx-agents"));
        assert_eq!(m[0], "-c");
        let (key, value) = m[1].split_once('=').unwrap();
        assert_eq!(key, "mcpServers.nyx-computer");
        assert_eq!(serde_json::from_str::<Value>(value).unwrap()["args"], json!(["computer", "--workspace", "w1"]));
    }
}
