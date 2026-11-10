//! One session's holder: owns the pty and the authoritative terminal state.
//!
//! The holder parses everything the program writes into its own
//! libghostty-vt terminal. It answers the program's terminal queries itself
//! (clients never do), tracks OSC 7501 status, and serves attachments:
//!
//! - `control`: snapshot, then live output; input and resizes are accepted.
//! - `observe`: snapshot, then live output; everything it sends is ignored.
//! - `events`: JSON status and lifecycle lines (the manager).
//!
//! A client that falls behind is not allowed to slow the program: its
//! backlog is dropped and it gets a fresh snapshot instead.

use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nyx_agents_core::{paths, sanitize, store};
use nyx_agents_proto::{
    Activity, Attach, AttachMode, Attached, BlockedOn, FrameKind, HolderEvent, ResizeMsg, Status, now_ms, read_frame, write_frame,
};
use nyx_pty::{Pty, Size, Spawn};
use nyx_term::{Colors, ProgramState, Rgb, StatusKind, StatusReport, Terminal, TerminalOptions};

enum Out {
    Frame(FrameKind, Vec<u8>),
    Line(String),
}

struct Client {
    id: u64,
    mode: AttachMode,
    tx: SyncSender<Out>,
    behind: Arc<AtomicBool>,
}

struct State {
    term: Terminal,
    clients: Vec<Client>,
    status: Status,
    cols: u16,
    rows: u16,
    title: String,
    exited: Option<(Option<i32>, Option<i32>)>,
    dirty: bool,
}

struct Shared {
    state: Mutex<State>,
    master: File,
    pid: u32,
    id: String,
    log: PathBuf,
}

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

pub fn run(args: &[String]) -> i32 {
    let split = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (opts, argv) = (&args[..split], args.get(split + 1..).unwrap_or(&[]));
    let Some(id) = arg(opts, "--id").filter(|i| store::valid_id(i)) else {
        eprintln!("hold: missing or invalid --id");
        return 2;
    };
    if argv.is_empty() {
        eprintln!("hold: nothing to run");
        return 2;
    }
    let cols: u16 = arg(opts, "--cols").and_then(|v| v.parse().ok()).unwrap_or(120).clamp(2, 1000);
    let rows: u16 = arg(opts, "--rows").and_then(|v| v.parse().ok()).unwrap_or(36).clamp(1, 500);
    let cwd = arg(opts, "--cwd").map(PathBuf::from);

    let dir = paths::session_dir(&id);
    if paths::private_dir(&dir).is_err() {
        return 1;
    }
    let log = dir.join("holder.log");
    let sock = paths::session_socket(&id);
    if let Some(parent) = sock.parent()
        && paths::private_dir(parent).is_err()
    {
        return 1;
    }
    let _ = std::fs::remove_file(&sock);
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            crate::log_to(&log, &format!("bind {}: {e}", sock.display()));
            return 1;
        }
    };

    let argv_os: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let env = [
        (OsStr::new("TERM"), OsStr::new("xterm-256color")),
        (OsStr::new("COLORTERM"), OsStr::new("truecolor")),
        (OsStr::new("TERM_PROGRAM"), OsStr::new("nyx-agents")),
        (OsStr::new("NYX_AGENTS_SESSION"), OsStr::new(id.as_str())),
    ];
    let pty = match Pty::spawn(&Spawn {
        argv: &argv_os,
        cwd: cwd.as_deref(),
        env: &env,
        env_remove: &[],
        size: Size { cols, rows, ..Default::default() },
    }) {
        Ok(p) => p,
        Err(e) => {
            crate::log_to(&log, &format!("spawn {}: {e}", argv[0]));
            let _ = std::fs::remove_file(&sock);
            return 1;
        }
    };
    let pid = pty.pid();
    crate::log_to(&log, &format!("started pid {pid}: {}", argv[0]));

    let Ok(mut term) = Terminal::new(&TerminalOptions { cols, rows, ..Default::default() }) else { return 1 };
    term.track_continuation(1 << 20);
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            term,
            clients: Vec::new(),
            status: Status::default(),
            cols,
            rows,
            title: String::new(),
            exited: None,
            dirty: false,
        }),
        master: pty.try_clone_master().expect("master"),
        pid,
        id: id.clone(),
        log,
    });

    let reader = pty.try_clone_master().expect("master");
    {
        let s = shared.clone();
        std::thread::spawn(move || read_loop(s, reader));
    }
    {
        let s = shared.clone();
        std::thread::spawn(move || snapshot_loop(s));
    }
    {
        let s = shared.clone();
        let sock = sock.clone();
        std::thread::spawn(move || wait_loop(s, pty, sock));
    }

    for stream in listener.incoming().flatten() {
        if !crate::peer::same_user(&stream) {
            continue;
        }
        let s = shared.clone();
        std::thread::spawn(move || serve_client(s, stream));
    }
    0
}

fn broadcast(st: &mut State, event: Option<&HolderEvent>, frame: Option<(FrameKind, &[u8])>) {
    let line = event.map(|e| serde_json::to_string(e).unwrap());
    st.clients.retain(|c| {
        let msg = match (c.mode, &line, frame) {
            (AttachMode::Events, Some(l), _) => Out::Line(l.clone()),
            (AttachMode::Control | AttachMode::Observe, _, Some((k, p))) => Out::Frame(k, p.to_vec()),
            _ => return true,
        };
        match c.tx.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                c.behind.store(true, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    });
}

fn activity(s: ProgramState) -> Activity {
    match s {
        ProgramState::Idle | ProgramState::Clear => Activity::Idle,
        ProgramState::Working => Activity::Working,
        ProgramState::Done => Activity::Done,
        ProgramState::Blocked => Activity::Blocked,
        ProgramState::Error => Activity::Error,
    }
}

/// Fold OSC 7501 reports for the program itself (the root record) into the
/// session status. A turn ends each time the program stops working.
fn apply_status(st: &mut State, reports: &[StatusReport], prompt_started: bool) -> bool {
    let mut changed = false;
    let mut set = |st: &mut State, act: Activity, kind: Option<BlockedOn>, progress: Option<u8>, msg: Option<String>| {
        let old = st.status.activity;
        if old == act && st.status.blocked_on == kind && st.status.message == msg && st.status.progress == progress {
            return;
        }
        if old == Activity::Working && act != Activity::Working {
            st.status.turn += 1;
        }
        if old != act {
            st.status.since = now_ms();
        }
        st.status.activity = act;
        st.status.blocked_on = kind;
        st.status.progress = progress;
        st.status.message = msg;
        changed = true;
    };
    for r in reports.iter().filter(|r| r.id.is_empty()) {
        let kind = match r.kind {
            StatusKind::Permission => Some(BlockedOn::Permission),
            StatusKind::Question => Some(BlockedOn::Question),
            StatusKind::Auth => Some(BlockedOn::Auth),
            StatusKind::None => None,
        };
        let msg = Some(sanitize::line(&r.message, 300)).filter(|m| !m.is_empty());
        set(st, activity(r.state), kind, r.progress, msg);
    }
    if prompt_started && matches!(st.status.activity, Activity::Working | Activity::Blocked) {
        set(st, Activity::Idle, None, None, None);
    }
    changed
}

fn read_loop(s: Arc<Shared>, mut master: File) {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match master.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let mut st = s.state.lock().unwrap();
        st.term.write(&buf[..n]);
        st.dirty = true;
        let fx = st.term.take_effects();
        if !fx.reply.is_empty() {
            let _ = (&s.master).write_all(&fx.reply);
        }
        broadcast(&mut st, None, Some((FrameKind::Output, &buf[..n])));
        if apply_status(&mut st, &fx.status, fx.prompt_started) {
            let ev = HolderEvent::Status { status: st.status.clone() };
            broadcast(&mut st, Some(&ev), None);
        }
        if fx.title_changed {
            let title = sanitize::line(&st.term.title(), 120);
            if title != st.title {
                st.title = title.clone();
                broadcast(&mut st, Some(&HolderEvent::Title { title }), None);
            }
        }
        if fx.pwd_changed {
            let pwd = st.term.pwd();
            let path = pwd.strip_prefix("file://").map(|p| p.find('/').map(|i| &p[i..]).unwrap_or(p)).unwrap_or(&pwd);
            let cwd = sanitize::line(path, 4096);
            broadcast(&mut st, Some(&HolderEvent::Cwd { cwd }), None);
        }
    }
}

fn save_snapshot(s: &Shared) {
    let bytes = {
        let mut st = s.state.lock().unwrap();
        if !st.dirty {
            return;
        }
        st.dirty = false;
        st.term.snapshot()
    };
    match bytes {
        Ok(b) => {
            let _ = paths::write_private(&store::snapshot_path(&s.id), &b);
        }
        Err(e) => crate::log_to(&s.log, &format!("snapshot: {e}")),
    }
}

fn snapshot_loop(s: Arc<Shared>) {
    loop {
        std::thread::sleep(Duration::from_secs(10));
        save_snapshot(&s);
    }
}

fn wait_loop(s: Arc<Shared>, mut pty: Pty, sock: PathBuf) {
    let status = pty.wait();
    use std::os::unix::process::ExitStatusExt;
    let (code, signal) = match &status {
        Ok(st) => (st.code(), st.signal()),
        Err(_) => (None, None),
    };
    // Let the reader drain the program's last output.
    std::thread::sleep(Duration::from_millis(200));
    {
        let mut st = s.state.lock().unwrap();
        st.exited = Some((code, signal));
        st.dirty = true;
        if matches!(st.status.activity, Activity::Working | Activity::Blocked) {
            apply_status(&mut st, &[], true);
            let ev = HolderEvent::Status { status: st.status.clone() };
            broadcast(&mut st, Some(&ev), None);
        }
        let payload = serde_json::json!({ "code": code, "signal": signal }).to_string();
        broadcast(&mut st, Some(&HolderEvent::Exited { code, signal }), Some((FrameKind::Exited, payload.as_bytes())));
    }
    save_snapshot(&s);
    crate::log_to(&s.log, &format!("exited code {code:?} signal {signal:?}"));
    // Give attached clients a moment to read the end, then go.
    std::thread::sleep(Duration::from_secs(2));
    let _ = std::fs::remove_file(&sock);
    std::process::exit(0);
}

fn parse_rgb(s: &Option<String>) -> Option<Rgb> {
    s.as_deref().and_then(Rgb::parse_hex)
}

fn serve_client(s: Arc<Shared>, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if (&mut reader).take(4096).read_line(&mut line).is_err() {
        return;
    }
    let Ok(attach) = serde_json::from_str::<Attach>(&line) else { return };
    let _ = stream.set_read_timeout(None);
    let mut writer = stream;

    let (tx, rx) = sync_channel::<Out>(256);
    let behind = Arc::new(AtomicBool::new(false));
    let id = NEXT_CLIENT.fetch_add(1, Ordering::Relaxed);
    {
        let mut st = s.state.lock().unwrap();
        if attach.attach == AttachMode::Control
            && let (Some(fg), Some(bg)) = (parse_rgb(&attach.foreground), parse_rgb(&attach.background))
        {
            let mut c = Colors::catppuccin_mocha();
            c.foreground = fg;
            c.background = bg;
            st.term.set_colors(&c);
        }
        let hello = Attached { ok: true, cols: st.cols, rows: st.rows, error: None };
        if writeln!(writer, "{}", serde_json::to_string(&hello).unwrap()).is_err() {
            return;
        }
        match attach.attach {
            AttachMode::Events => {
                let mut initial = vec![
                    HolderEvent::Started { pid: s.pid },
                    HolderEvent::Grid { cols: st.cols, rows: st.rows },
                    HolderEvent::Status { status: st.status.clone() },
                ];
                if !st.title.is_empty() {
                    initial.push(HolderEvent::Title { title: st.title.clone() });
                }
                if let Some((code, signal)) = st.exited {
                    initial.push(HolderEvent::Exited { code, signal });
                }
                for e in initial {
                    let _ = tx.try_send(Out::Line(serde_json::to_string(&e).unwrap()));
                }
            }
            AttachMode::Control | AttachMode::Observe => {
                let snap = st.term.snapshot().unwrap_or_default();
                let _ = tx.try_send(Out::Frame(FrameKind::Snapshot, snap));
                if let Some((code, signal)) = st.exited {
                    let p = serde_json::json!({ "code": code, "signal": signal }).to_string().into_bytes();
                    let _ = tx.try_send(Out::Frame(FrameKind::Exited, p));
                }
            }
        }
        st.clients.push(Client { id, mode: attach.attach, tx, behind: behind.clone() });
    }

    {
        let s = s.clone();
        let behind = behind.clone();
        let mode = attach.attach;
        std::thread::spawn(move || write_loop(s, writer, rx, behind, mode));
    }

    match attach.attach {
        AttachMode::Events => {
            let mut sink = [0u8; 256];
            while matches!(reader.read(&mut sink), Ok(n) if n > 0) {}
        }
        mode => {
            while let Ok((kind, payload)) = read_frame(&mut reader) {
                // Observers are read-only: nothing they send reaches the program.
                if mode != AttachMode::Control {
                    continue;
                }
                match FrameKind::from_u8(kind) {
                    Some(FrameKind::Input) => {
                        let exited = s.state.lock().unwrap().exited.is_some();
                        if !exited {
                            let _ = (&s.master).write_all(&payload);
                        }
                    }
                    Some(FrameKind::Resize) => {
                        if let Ok(r) = serde_json::from_slice::<ResizeMsg>(&payload) {
                            resize(&s, r);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    s.state.lock().unwrap().clients.retain(|c| c.id != id);
}

fn resize(s: &Shared, r: ResizeMsg) {
    let (cols, rows) = (r.cols.clamp(2, 1000), r.rows.clamp(1, 500));
    let mut st = s.state.lock().unwrap();
    if (st.cols, st.rows) == (cols, rows) {
        return;
    }
    let cw = (r.width / cols as u32).max(1);
    let ch = (r.height / rows as u32).max(1);
    let _ = st.term.resize(cols, rows, cw, ch);
    let _ = nyx_pty::resize_master(&s.master, Size { cols, rows, width_px: r.width as u16, height_px: r.height as u16 });
    st.cols = cols;
    st.rows = rows;
    st.dirty = true;
    // Every viewer re-syncs from a snapshot at the new grid.
    for c in st.clients.iter().filter(|c| c.mode != AttachMode::Events) {
        c.behind.store(true, Ordering::Relaxed);
    }
    broadcast(&mut st, Some(&HolderEvent::Grid { cols, rows }), None);
    // Wake writers that have nothing queued so they resync now.
    for c in &st.clients {
        if c.mode != AttachMode::Events {
            let _ = c.tx.try_send(Out::Frame(FrameKind::Output, Vec::new()));
        }
    }
}

fn write_loop(s: Arc<Shared>, mut w: UnixStream, rx: Receiver<Out>, behind: Arc<AtomicBool>, mode: AttachMode) {
    while let Ok(msg) = rx.recv() {
        if behind.swap(false, Ordering::Relaxed) {
            // Replace the backlog with the current state. Draining under the
            // lock keeps later output strictly after what is sent here.
            let (catch_up, exited) = {
                let st = s.state.lock().unwrap();
                while rx.try_recv().is_ok() {}
                let exited = st.exited.map(|(code, signal)| serde_json::json!({ "code": code, "signal": signal }).to_string());
                let catch_up = match mode {
                    // Events are state, not a stream: the current status will do.
                    AttachMode::Events => {
                        let line = serde_json::to_string(&HolderEvent::Status { status: st.status.clone() }).unwrap();
                        Out::Line(line)
                    }
                    _ => Out::Frame(FrameKind::Snapshot, st.term.snapshot().unwrap_or_default()),
                };
                (catch_up, exited)
            };
            let ok = match catch_up {
                Out::Frame(k, p) => write_frame(&mut w, k, &p).is_ok(),
                Out::Line(l) => writeln!(w, "{l}").is_ok(),
            };
            if !ok {
                return;
            }
            if let Some(p) = exited.filter(|_| mode != AttachMode::Events)
                && write_frame(&mut w, FrameKind::Exited, p.as_bytes()).is_err()
            {
                return;
            }
            continue;
        }
        let ok = match msg {
            Out::Frame(FrameKind::Output, p) if p.is_empty() => true,
            Out::Frame(k, p) => write_frame(&mut w, k, &p).is_ok(),
            Out::Line(l) => writeln!(w, "{l}").is_ok(),
        };
        if !ok {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            term: Terminal::new(&TerminalOptions::default()).unwrap(),
            clients: vec![],
            status: Status::default(),
            cols: 80,
            rows: 24,
            title: String::new(),
            exited: None,
            dirty: false,
        }
    }

    fn report(state: ProgramState) -> StatusReport {
        StatusReport {
            state,
            kind: StatusKind::None,
            progress: None,
            id: String::new(),
            app: "claude-code".into(),
            title: String::new(),
            message: String::new(),
        }
    }

    #[test]
    fn turns_count_each_time_work_stops() {
        let mut st = state();
        assert!(apply_status(&mut st, &[report(ProgramState::Working)], false));
        assert_eq!((st.status.activity, st.status.turn), (Activity::Working, 0));
        apply_status(&mut st, &[report(ProgramState::Done)], false);
        assert_eq!((st.status.activity, st.status.turn), (Activity::Done, 1));
        apply_status(&mut st, &[report(ProgramState::Working)], false);
        apply_status(&mut st, &[], true);
        assert_eq!((st.status.activity, st.status.turn), (Activity::Idle, 2), "a new prompt ends the turn");
        // Records for sub-tasks don't change the session's own status.
        let mut child = report(ProgramState::Blocked);
        child.id = "build".into();
        assert!(!apply_status(&mut st, &[child], false));
    }
}
