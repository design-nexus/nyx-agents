//! The manager: session records, harness discovery, holders, events and
//! the AI Hub snapshot.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nyx_agents_core::{adapters, git, hub, new_id, paths, sanitize, store};
use nyx_agents_proto::{
    Activity, Attach, AttachMode, Attached, Call, Event, Harness, HolderEvent, Launch, Lifecycle, PROTOCOL_VERSION, Project,
    Request, Response, RpcError, Session, Status, now_ms,
};
use serde_json::{Value, json};

type Writer = Arc<Mutex<UnixStream>>;

pub struct Daemon {
    sessions: Mutex<HashMap<String, Session>>,
    /// The session on screen in the app, if its window is focused.
    viewing: Mutex<Option<String>>,
    /// Sessions the user asked to stop (their exit is not news).
    stopping: Mutex<Vec<String>>,
    notifier: Mutex<Option<Arc<crate::notify::Notifier>>>,
    subscribers: Mutex<Vec<Writer>>,
    harnesses: Mutex<Vec<Harness>>,
    log: PathBuf,
}

impl Daemon {
    pub fn log(&self, line: &str) {
        crate::log_to(&self.log, line);
    }

    fn emit(&self, ev: &Event) {
        let line = format!("{}\n", serde_json::to_string(ev).unwrap());
        self.subscribers.lock().unwrap().retain(|w| w.lock().unwrap().write_all(line.as_bytes()).is_ok());
    }

    /// Store, publish and mirror one session after a change.
    fn changed(&self, s: &Session) {
        if let Err(e) = store::save(s) {
            self.log(&format!("save {}: {e}", s.id));
        }
        self.emit(&Event::Session { session: Box::new(s.clone()) });
        self.write_hub();
    }

    fn write_hub(&self) {
        let all: Vec<Session> = self.sessions.lock().unwrap().values().cloned().collect();
        if let Err(e) = hub::write(&all) {
            self.log(&format!("hub: {e}"));
        }
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Session)) -> Option<Session> {
        let s = {
            let mut map = self.sessions.lock().unwrap();
            let s = map.get_mut(id)?;
            f(s);
            s.clone()
        };
        self.changed(&s);
        Some(s)
    }

    /// Tell the user about a change worth their attention, unless they are
    /// looking at that session already.
    fn notice(&self, before: &Session, after: &Session) {
        use nyx_agents_proto::Activity as A;
        if self.viewing.lock().unwrap().as_deref() == Some(after.id.as_str()) || !crate::notify::enabled() {
            return;
        }
        let Some(n) = self.notifier.lock().unwrap().clone() else { return };
        let who = self.harness(&after.harness).map(|h| h.name).unwrap_or_else(|| after.harness.clone());
        // Say which session it is, so one agent can't pass for another.
        let body = |fallback: &str| {
            let msg = after.status.message.clone().unwrap_or_else(|| fallback.to_string());
            format!("{}\n{msg}", after.title)
        };
        let (b, a) = (before.status.activity, after.status.activity);
        if after.lifecycle == Lifecycle::Running && a != b {
            match a {
                A::Blocked => n.send(&after.id, &format!("{who} needs you"), &body("Waiting for your answer"), true),
                A::Done => n.send(&after.id, &format!("{who} finished"), &body("Ready for you to look at"), false),
                A::Error => n.send(&after.id, &format!("{who} hit an error"), &body("It stopped working"), true),
                _ => {}
            }
        }
        if before.lifecycle == Lifecycle::Running && after.lifecycle == Lifecycle::Exited {
            let asked = self.stopping.lock().unwrap().iter().any(|s| *s == after.id);
            if !asked && after.exit_code.is_some_and(|c| c != 0) && after.harness != "shell" {
                let code = after.exit_code.unwrap_or_default();
                n.send(&after.id, &format!("{who} ended unexpectedly"), &format!("{}\nExit code {code}", after.title), true);
            }
        }
    }

    fn harness(&self, id: &str) -> Option<Harness> {
        let mut cache = self.harnesses.lock().unwrap();
        if cache.is_empty() {
            *cache = adapters::harnesses(false);
        }
        cache.iter().find(|h| h.id == id).cloned()
    }

    fn resumable(&self, s: &Session) -> bool {
        let Some(hs) = &s.harness_session else { return false };
        if !self.harness(&s.harness).is_some_and(|h| h.capabilities.resume) {
            return false;
        }
        // Each harness only has something to resume once a conversation began.
        match s.harness.as_str() {
            "claude" => adapters::claude_transcript(&s.cwd, hs).exists(),
            "codex" => nyx_agents_core::codex::has_session(hs, s.created),
            "grok" => std::fs::read_dir(paths::home().join(".grok/sessions"))
                .map(|it| it.flatten().any(|e| e.path().join(hs).is_dir()))
                .unwrap_or(false),
            _ => false,
        }
    }
}

pub fn run() -> i32 {
    let runtime = paths::runtime_dir();
    if paths::private_dir(&runtime).is_err() || paths::private_dir(&paths::sessions_dir()).is_err() {
        eprintln!("nyx-agentd: cannot create {}", runtime.display());
        return 1;
    }
    let listener = match listener() {
        Ok(Some(l)) => l,
        Ok(None) => return 0,
        Err(e) => {
            eprintln!("nyx-agentd: {e}");
            return 1;
        }
    };
    let d = Arc::new(Daemon {
        sessions: Mutex::new(HashMap::new()),
        viewing: Mutex::new(None),
        stopping: Mutex::new(Vec::new()),
        notifier: Mutex::new(None),
        subscribers: Mutex::new(Vec::new()),
        harnesses: Mutex::new(Vec::new()),
        log: paths::state_dir().join("nyx-agentd.log"),
    });
    d.log(&format!("started pid {}", std::process::id()));

    // Sessions from before: reattach to holders that are still running.
    for mut s in store::load_all() {
        let id = s.id.clone();
        let live = matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting);
        if live && paths::session_socket(&id).exists() && UnixStream::connect(paths::session_socket(&id)).is_ok() {
            d.sessions.lock().unwrap().insert(id.clone(), s);
            let d2 = d.clone();
            std::thread::spawn(move || monitor(d2, id));
            continue;
        }
        if live {
            s.lifecycle = Lifecycle::Lost;
            s.ended.get_or_insert(now_ms());
            s.status.activity = Activity::Unknown;
            s.resumable = d.resumable(&s);
            let _ = store::save(&s);
        }
        d.sessions.lock().unwrap().insert(id, s);
    }
    d.write_hub();

    {
        let d = d.clone();
        std::thread::spawn(move || {
            let list = adapters::harnesses(true);
            *d.harnesses.lock().unwrap() = list.clone();
            d.emit(&Event::Harnesses { harnesses: list });
        });
    }

    // Kept for the process lifetime; dropping it would release the name.
    let _bus = if std::env::var_os("NYX_AGENTS_NO_DBUS").is_some() { None } else { crate::control::serve(d.clone()) };
    if std::env::var_os("NYX_AGENTS_NO_DBUS").is_none() {
        *d.notifier.lock().unwrap() = crate::notify::Notifier::new();
    }

    for stream in listener.incoming().flatten() {
        if !crate::peer::same_user(&stream) {
            continue;
        }
        let d = d.clone();
        std::thread::spawn(move || client(d, stream));
    }
    0
}

/// The control socket: handed over by systemd, or bound here. `None` when
/// another manager already answers on it.
fn listener() -> std::io::Result<Option<UnixListener>> {
    let pid_ok = std::env::var("LISTEN_PID").ok().and_then(|p| p.parse::<u32>().ok()) == Some(std::process::id());
    if pid_ok && std::env::var("LISTEN_FDS").as_deref() == Ok("1") {
        return Ok(Some(unsafe { UnixListener::from_raw_fd(3) }));
    }
    let sock = paths::control_socket();
    if UnixStream::connect(&sock).is_ok() {
        eprintln!("nyx-agentd: already running on {}", sock.display());
        return Ok(None);
    }
    let _ = std::fs::remove_file(&sock);
    let l = UnixListener::bind(&sock)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o600))?;
    Ok(Some(l))
}

fn client(d: Arc<Daemon>, stream: UnixStream) {
    let writer: Writer = Arc::new(Mutex::new(stream.try_clone().expect("clone")));
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.len() > 1 << 20 {
            break;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let id = req.id;
                let subscribe = matches!(req.call, Call::Subscribe);
                let r = handle(&d, req.call);
                if subscribe {
                    d.subscribers.lock().unwrap().push(writer.clone());
                }
                match r {
                    Ok(v) => Response { id, result: Some(v), error: None },
                    Err(e) => Response { id, result: None, error: Some(e) },
                }
            }
            Err(e) => Response { id: 0, result: None, error: Some(err("bad-request", &e.to_string())) },
        };
        let out = format!("{}\n", serde_json::to_string(&resp).unwrap());
        if writer.lock().unwrap().write_all(out.as_bytes()).is_err() {
            break;
        }
    }
}

fn err(code: &str, message: &str) -> RpcError {
    RpcError { code: code.into(), message: message.into() }
}

pub fn handle(d: &Arc<Daemon>, call: Call) -> Result<Value, RpcError> {
    let get = |id: &str| d.sessions.lock().unwrap().get(id).cloned().ok_or_else(|| err("not-found", "no such session"));
    Ok(match call {
        Call::Hello { .. } => json!({ "daemon": "nyx-agentd", "version": PROTOCOL_VERSION, "pid": std::process::id() }),
        Call::ListHarnesses { refresh } => {
            if refresh {
                let list = adapters::harnesses(true);
                *d.harnesses.lock().unwrap() = list;
            }
            let mut list = d.harnesses.lock().unwrap().clone();
            if list.is_empty() {
                list = adapters::harnesses(false);
            }
            serde_json::to_value(list).unwrap()
        }
        Call::ListSessions => {
            let mut all: Vec<Session> = d.sessions.lock().unwrap().values().cloned().collect();
            all.sort_by_key(|s| std::cmp::Reverse(s.created));
            serde_json::to_value(all).unwrap()
        }
        Call::GetSession { id } => serde_json::to_value(get(&id)?).unwrap(),
        Call::Launch(l) => serde_json::to_value(launch(d, l)?).unwrap(),
        Call::Resume { id } => serde_json::to_value(resume(d, &id)?).unwrap(),
        Call::Stop { id, force } => {
            let s = get(&id)?;
            let pid = s
                .pid
                .filter(|_| s.lifecycle == Lifecycle::Running)
                .ok_or_else(|| err("not-running", "session is not running"))?;
            d.stopping.lock().unwrap().push(id.clone());
            // The agent leads its own process group on its pty.
            let sig = if force { libc::SIGKILL } else { libc::SIGHUP };
            unsafe { libc::kill(-(pid as i32), sig) };
            json!({ "signalled": pid })
        }
        Call::Forget { id } => {
            let s = get(&id)?;
            if matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting) {
                return Err(err("running", "stop the session first"));
            }
            store::remove(&id).map_err(|e| err("io", &e.to_string()))?;
            d.sessions.lock().unwrap().remove(&id);
            d.emit(&Event::Removed { id });
            d.write_hub();
            json!({})
        }
        Call::Rename { id, title } => {
            let title = sanitize::line(&title, 120);
            serde_json::to_value(d.update(&id, |s| s.title = title).ok_or_else(|| err("not-found", "no such session"))?).unwrap()
        }
        Call::ListProjects => {
            let mut seen = HashMap::new();
            for s in d.sessions.lock().unwrap().values() {
                if let (Some(pid), Some(wt)) = (&s.project, &s.worktree) {
                    let root = project_root(&s.cwd).unwrap_or_else(|| PathBuf::from(&wt.root));
                    seen.entry(pid.clone()).or_insert_with(|| Project {
                        id: pid.clone(),
                        name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                        path: root.display().to_string(),
                    });
                }
            }
            serde_json::to_value(seen.into_values().collect::<Vec<_>>()).unwrap()
        }
        Call::Viewing { id } => {
            *d.viewing.lock().unwrap() = id;
            json!({})
        }
        // Agent workspaces arrive with the compositor's API; none exist yet.
        Call::ListWorkspaces => json!([]),
        Call::Subscribe => json!({ "subscribed": true }),
    })
}

fn project_root(cwd: &str) -> Option<PathBuf> {
    git::context(Path::new(cwd)).map(|(_, main)| main)
}

fn project_id(root: &Path) -> String {
    // FNV-1a of the path: stable across restarts, no need to store a table.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in root.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn launch(d: &Arc<Daemon>, l: Launch) -> Result<Session, RpcError> {
    let h = d.harness(&l.harness).ok_or_else(|| err("unknown-harness", &format!("unknown harness {}", l.harness)))?;
    let cwd = Path::new(&l.cwd).canonicalize().map_err(|_| err("bad-cwd", "working directory does not exist"))?;
    if !cwd.is_dir() {
        return Err(err("bad-cwd", "working directory is not a folder"));
    }
    let plan = adapters::launch(&h, &l.args).map_err(|e| err("not-installed", &e))?;
    let ctx = git::context(&cwd);
    let folder = cwd.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "/".into());
    let s = Session {
        id: new_id(),
        harness: h.id.clone(),
        title: l.title.map(|t| sanitize::line(&t, 120)).unwrap_or_else(|| format!("{} · {folder}", h.name)),
        project: ctx.as_ref().map(|(_, root)| project_id(root)),
        workspace: l.workspace,
        cwd: cwd.display().to_string(),
        worktree: ctx.map(|(wt, _)| wt),
        harness_session: plan.harness_session.clone(),
        lifecycle: Lifecycle::Starting,
        status: Status::default(),
        exit_code: None,
        pid: None,
        created: now_ms(),
        ended: None,
        cols: l.cols.clamp(2, 1000),
        rows: l.rows.clamp(1, 500),
        resumable: false,
        subagents: Vec::new(),
    };
    d.sessions.lock().unwrap().insert(s.id.clone(), s.clone());
    d.changed(&s);
    start_holder(d, &s, &plan.argv)?;
    Ok(s)
}

fn resume(d: &Arc<Daemon>, id: &str) -> Result<Session, RpcError> {
    let s = d.sessions.lock().unwrap().get(id).cloned().ok_or_else(|| err("not-found", "no such session"))?;
    if matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting) {
        return Err(err("running", "session is already running"));
    }
    let h = d.harness(&s.harness).ok_or_else(|| err("unknown-harness", "harness not found"))?;
    let plan = adapters::resume(&h, &s).map_err(|e| err("not-resumable", &e))?;
    let s = d
        .update(id, |s| {
            s.lifecycle = Lifecycle::Starting;
            s.status = Status::default();
            s.exit_code = None;
            s.ended = None;
            s.pid = None;
            s.resumable = false;
        })
        .unwrap();
    start_holder(d, &s, &plan.argv)?;
    Ok(s)
}

fn start_holder(d: &Arc<Daemon>, s: &Session, argv: &[String]) -> Result<(), RpcError> {
    let exe = std::env::current_exe().map_err(|e| err("io", &e.to_string()))?;
    let mut cmd = Command::new(exe);
    cmd.arg("hold")
        .args(["--id", &s.id, "--cols", &s.cols.to_string(), "--rows", &s.rows.to_string(), "--cwd", &s.cwd, "--"])
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Its own session, so it outlives the manager (the unit kills only the
    // main process on stop).
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| err("spawn", &e.to_string()))?;
    d.log(&format!("holder for {} pid {}", s.id, child.id()));
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let sock = paths::session_socket(&s.id);
    let start = Instant::now();
    while UnixStream::connect(&sock).is_err() {
        if start.elapsed() > Duration::from_secs(5) {
            d.update(&s.id, |s| {
                s.lifecycle = Lifecycle::Exited;
                s.ended = Some(now_ms());
            });
            return Err(err("spawn", "the session did not start; see holder.log"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let d2 = d.clone();
    let id = s.id.clone();
    std::thread::spawn(move || monitor(d2, id));
    Ok(())
}

/// Learn a Codex session's own id from the rollout Codex writes for it.
/// Read only; inotify wakes it, the timeout only notices the session ending.
fn watch_codex(d: Arc<Daemon>, id: String) {
    use nyx_agents_core::{codex, inotify::Watcher};
    let Some((cwd, since)) = d
        .sessions
        .lock()
        .unwrap()
        .get(&id)
        .and_then(|s| (s.harness == "codex" && s.harness_session.is_none()).then(|| (s.cwd.clone(), s.created)))
    else {
        return;
    };
    let Ok(w) = Watcher::new() else { return };
    loop {
        for dir in codex::day_dirs(since) {
            w.add(&dir);
            if let Some(parent) = dir.parent() {
                w.add(parent);
            }
        }
        let taken: Vec<String> = d.sessions.lock().unwrap().values().filter_map(|s| s.harness_session.clone()).collect();
        if let Some(found) = codex::find_session(&cwd, since, &taken) {
            d.update(&id, |s| s.harness_session = Some(found));
            return;
        }
        let live =
            d.sessions.lock().unwrap().get(&id).is_some_and(|s| matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting));
        if !live {
            return;
        }
        w.wait(Duration::from_secs(2));
    }
}

/// Follow a Claude session's subagents from its transcripts while it runs.
/// Read only; inotify wakes it, the timeout only notices the session ending.
fn watch_subagents(d: Arc<Daemon>, id: String) {
    use nyx_agents_core::{claude, inotify::Watcher};
    let Some((cwd, hs)) = d
        .sessions
        .lock()
        .unwrap()
        .get(&id)
        .and_then(|s| (s.harness == "claude").then(|| s.harness_session.clone().map(|h| (s.cwd.clone(), h))).flatten())
    else {
        return;
    };
    let Ok(w) = Watcher::new() else { return };
    let parent = adapters::claude_transcript(&cwd, &hs);
    let mut subs = claude::Subagents::default();
    loop {
        if let Some(dir) = parent.parent() {
            w.add(dir);
        }
        w.add(&parent.with_extension(""));
        w.add(&claude::subagents_dir(&cwd, &hs));
        if let Some(list) = subs.refresh(&cwd, &hs) {
            d.update(&id, |s| s.subagents = list);
        }
        let live =
            d.sessions.lock().unwrap().get(&id).is_some_and(|s| matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting));
        if !live {
            return;
        }
        w.wait(Duration::from_secs(2));
    }
}

/// Follow one holder's events until it goes away.
fn monitor(d: Arc<Daemon>, id: String) {
    {
        let d = d.clone();
        let id = id.clone();
        let harness = d.sessions.lock().unwrap().get(&id).map(|s| s.harness.clone()).unwrap_or_default();
        match harness.as_str() {
            "claude" => std::thread::spawn(move || watch_subagents(d, id)),
            "codex" => std::thread::spawn(move || watch_codex(d, id)),
            _ => std::thread::spawn(|| {}),
        };
    }
    let ended_cleanly = (|| -> std::io::Result<bool> {
        let mut stream = UnixStream::connect(paths::session_socket(&id))?;
        let hello = Attach { attach: AttachMode::Events, foreground: None, background: None };
        writeln!(stream, "{}", serde_json::to_string(&hello).unwrap())?;
        let mut lines = BufReader::new(stream).lines();
        let first = lines.next().transpose()?.unwrap_or_default();
        if !serde_json::from_str::<Attached>(&first).map(|a| a.ok).unwrap_or(false) {
            return Ok(false);
        }
        for line in lines {
            let Ok(ev) = serde_json::from_str::<HolderEvent>(&line?) else { continue };
            let exited = matches!(ev, HolderEvent::Exited { .. });
            let resumable = d.sessions.lock().unwrap().get(&id).map(|s| d.resumable(s)).unwrap_or(false);
            let before = d.sessions.lock().unwrap().get(&id).cloned();
            let after = d.update(&id, |s| match ev {
                HolderEvent::Started { pid } => {
                    s.pid = Some(pid);
                    if s.lifecycle == Lifecycle::Starting {
                        s.lifecycle = Lifecycle::Running;
                    }
                }
                HolderEvent::Status { status } => s.status = status,
                HolderEvent::Grid { cols, rows } => {
                    s.cols = cols;
                    s.rows = rows;
                }
                HolderEvent::Title { .. } | HolderEvent::Cwd { .. } => {}
                HolderEvent::Exited { code, signal } => {
                    s.lifecycle = Lifecycle::Exited;
                    s.exit_code = code.or(signal.map(|sig| 128 + sig));
                    s.ended = Some(now_ms());
                    s.resumable = resumable;
                }
            });
            if let (Some(b), Some(a)) = (before, after) {
                d.notice(&b, &a);
            }
            if exited {
                d.stopping.lock().unwrap().retain(|s| *s != id);
                return Ok(true);
            }
        }
        Ok(false)
    })()
    .unwrap_or(false);
    if !ended_cleanly {
        let resumable = d.sessions.lock().unwrap().get(&id).map(|s| d.resumable(s)).unwrap_or(false);
        d.update(&id, |s| {
            if matches!(s.lifecycle, Lifecycle::Running | Lifecycle::Starting) {
                s.lifecycle = Lifecycle::Lost;
                s.ended = Some(now_ms());
                s.resumable = resumable;
            }
        });
    }
}
