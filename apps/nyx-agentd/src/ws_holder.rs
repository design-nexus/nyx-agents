//! One agent workspace's holder:
//!
//!   nyx-agentd ws-hold --id ID --name NAME --width W --height H --scale S
//!
//! It owns the workspace's sway, private bus and programs, in a session of its
//! own, so a workspace outlives the manager the way an agent's terminal does.
//! It writes `holder.json` beside its socket and answers one JSON request per
//! line on `holder.sock`; the manager reattaches through them after a restart.
//! When sway ends, or on `stop`, it ends everything and removes its folders.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nyx_agent_workspace::{Clipboard, Instance, Options, base64, unbase64};
use nyx_agents_core::paths;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// What the manager needs to reattach to a workspace.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HolderState {
    pub id: String,
    pub name: String,
    pub created: u64,
    pub pid: u32,
    pub wayland: PathBuf,
    pub swaysock: PathBuf,
    pub display: Option<String>,
    pub options: Options,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Launch {
        argv: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Resize {
        options: Options,
    },
    /// `items`: mime type and base64 bytes.
    ClipboardSet {
        items: Vec<(String, String)>,
    },
    ClipboardGet {
        types: Vec<String>,
    },
    Stop,
}

pub fn runtime_root() -> PathBuf {
    paths::runtime_dir().join("ws")
}

pub fn data_root() -> PathBuf {
    paths::state_dir().join("workspaces")
}

pub fn state_file(dir: &Path) -> PathBuf {
    dir.join("holder.json")
}

pub fn socket(dir: &Path) -> PathBuf {
    dir.join("holder.sock")
}

/// A holder process that is still running (and is a holder, not a reused pid).
pub fn alive(pid: u32) -> bool {
    pid > 1
        && std::fs::read(format!("/proc/{pid}/cmdline")).map(|c| c.split(|b| *b == 0).any(|a| a == b"ws-hold")).unwrap_or(false)
}

/// Send one request to a holder and read its answer.
pub fn call(dir: &Path, req: &Request) -> Result<Value, String> {
    let mut s = UnixStream::connect(socket(dir)).map_err(|e| format!("workspace not reachable: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(20))).ok();
    writeln!(s, "{}", serde_json::to_string(req).unwrap()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|_| "the workspace gave no answer".to_string())?;
    match v.get("error").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn write_state(dir: &Path, st: &HolderState) {
    let _ = paths::write_private(&state_file(dir), &serde_json::to_vec_pretty(st).unwrap());
}

pub fn run(args: &[String]) -> i32 {
    let say = |v: Value| {
        let mut out = std::io::stdout();
        let _ = writeln!(out, "{v}");
        let _ = out.flush();
    };
    let Some(id) = opt(args, "--id").filter(|i| nyx_agents_core::store::valid_id(i)).map(String::from) else {
        say(json!({ "error": "ws-hold needs --id" }));
        return 2;
    };
    let num = |k: &str, d: f64| opt(args, k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
    let def = Options::default();
    let options = Options {
        width: num("--width", def.width as f64) as u32,
        height: num("--height", def.height as f64) as u32,
        scale: num("--scale", def.scale),
    };
    let name = opt(args, "--name").unwrap_or("Workspace").to_string();
    for d in [runtime_root(), data_root()] {
        if let Err(e) = paths::private_dir(&d) {
            say(json!({ "error": format!("cannot create {}: {e}", d.display()) }));
            return 1;
        }
    }
    let inst = match Instance::start(&id, &runtime_root(), &data_root(), options) {
        Ok(i) => Arc::new(i),
        Err(e) => {
            say(json!({ "error": e }));
            return 1;
        }
    };
    let listener = match UnixListener::bind(socket(&inst.dir)) {
        Ok(l) => l,
        Err(e) => {
            say(json!({ "error": format!("cannot listen: {e}") }));
            return 1;
        }
    };
    let state = HolderState {
        id: id.clone(),
        name,
        created: nyx_agents_proto::now_ms(),
        pid: std::process::id(),
        wayland: inst.wayland.clone(),
        swaysock: inst.swaysock.clone(),
        display: inst.display.clone(),
        options,
    };
    write_state(&inst.dir, &state);
    let clipboard = Clipboard::connect(&inst.wayland).ok().map(Arc::new);
    say(serde_json::to_value(&state).unwrap());
    // Nothing reads our output any more; keep writes from failing.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if null >= 0 {
            libc::dup2(null, 1);
            libc::close(null);
        }
    }

    {
        let inst = inst.clone();
        std::thread::spawn(move || {
            while inst.alive() {
                std::thread::sleep(Duration::from_secs(1));
            }
            inst.stop();
            std::process::exit(0);
        });
    }
    let state = Arc::new(std::sync::Mutex::new(state));
    for stream in listener.incoming().flatten() {
        if !crate::peer::same_user(&stream) {
            continue;
        }
        let (inst, clipboard, state) = (inst.clone(), clipboard.clone(), state.clone());
        std::thread::spawn(move || serve(stream, &inst, clipboard.as_deref(), &state));
    }
    0
}

fn serve(stream: UnixStream, inst: &Instance, clipboard: Option<&Clipboard>, state: &std::sync::Mutex<HolderState>) {
    let Ok(mut w) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        let reply = match serde_json::from_str::<Request>(&line) {
            Err(e) => json!({ "error": format!("bad request: {e}") }),
            Ok(req) => answer(req, inst, clipboard, state).unwrap_or_else(|e| json!({ "error": e })),
        };
        if writeln!(w, "{reply}").is_err() {
            return;
        }
    }
}

fn answer(
    req: Request,
    inst: &Instance,
    clipboard: Option<&Clipboard>,
    state: &std::sync::Mutex<HolderState>,
) -> Result<Value, String> {
    let clip = || clipboard.ok_or_else(|| "the workspace has no clipboard access".to_string());
    Ok(match req {
        Request::Ping => json!({ "ok": true }),
        Request::Launch { argv, cwd } => json!({ "pid": inst.launch(&argv, cwd.as_deref())? }),
        Request::Resize { options } => {
            inst.resize(options)?;
            let mut st = state.lock().unwrap();
            st.options = options;
            write_state(&inst.dir, &st);
            json!({ "ok": true })
        }
        Request::ClipboardSet { items } => {
            let items = items
                .into_iter()
                .map(|(m, d)| unbase64(&d).map(|d| (m, d)).ok_or_else(|| "clipboard data is not base64".to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            clip()?.set(items)?;
            json!({ "ok": true })
        }
        Request::ClipboardGet { types } => {
            let types: Vec<&str> = types.iter().map(String::as_str).collect();
            match clip()?.get(&types)? {
                Some((mime, data)) => json!({ "mime": mime, "data": base64(&data) }),
                None => json!({ "mime": null }),
            }
        }
        Request::Stop => {
            inst.stop();
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                std::process::exit(0);
            });
            json!({ "ok": true })
        }
    })
}
