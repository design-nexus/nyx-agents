use std::io::{Read, Write};

use nyx_agents_client::{Attachment, Client, FrameKind};
use nyx_agents_proto::{Activity, AttachMode, Launch, Lifecycle, ResizeMsg, Session, now_ms};
use nyx_term::{Terminal, TerminalOptions};

fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn positional(args: &[String]) -> Option<&str> {
    args.iter().skip(1).find(|a| !a.starts_with("--")).map(String::as_str)
}

pub fn run(args: &[String]) -> i32 {
    let Some(cmd) = args.first().map(String::as_str) else {
        eprintln!("usage: nyx-agents harnesses|ls|status|launch|attach|resume|stop|forget|hub (see --help)");
        return 2;
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        print!(
            "{}",
            include_str!("main.rs")
                .lines()
                .skip(2)
                .take(9)
                .map(|l| format!("{}\n", l.trim_start_matches("//!")))
                .collect::<String>()
        );
        return 0;
    }
    let result = (|| -> Result<i32, Box<dyn std::error::Error>> {
        let mut c = Client::connect("nyx-agents-cli")?;
        let json = flag(args, "--json");
        match cmd {
            "harnesses" => {
                let list = c.harnesses(flag(args, "--refresh"))?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                } else {
                    for h in list {
                        let state = match (&h.path, &h.version) {
                            (None, _) => "not installed".to_string(),
                            (Some(_), Some(v)) => v.clone(),
                            (Some(p), None) => p.clone(),
                        };
                        println!(
                            "{:<12} {:<16} {:<9} {:?}  {}",
                            h.id,
                            h.name,
                            format!("{:?}", h.tier).to_lowercase(),
                            h.auth,
                            state
                        );
                    }
                }
            }
            "ls" => {
                let list = c.sessions()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&list)?);
                } else if list.is_empty() {
                    println!("No sessions.");
                } else {
                    for s in list {
                        println!("{}", line(&s));
                    }
                }
            }
            "status" => {
                let id = c.resolve(positional(args).ok_or("status needs a session id")?)?;
                let s = c.session(&id)?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&s)?);
                } else {
                    println!("{}", line(&s));
                    println!("  cwd      {}", s.cwd);
                    if let Some(w) = &s.worktree {
                        println!(
                            "  branch   {}{}",
                            w.branch.as_deref().unwrap_or("?"),
                            if w.linked { " (worktree)" } else { "" }
                        );
                    }
                    if let Some(m) = &s.status.message {
                        println!("  message  {m}");
                    }
                    println!("  turn     {}", s.status.turn);
                    for a in &s.subagents {
                        let state = format!("{:?}", a.state).to_lowercase();
                        println!("  subagent {:<10} {:<9} {}", a.name, state, a.description.as_deref().unwrap_or(""));
                    }
                    if s.resumable {
                        println!("  resumable: nyx-agents resume {}", &s.id[..8]);
                    }
                }
            }
            "launch" => {
                let harness = positional(args).ok_or("launch needs a harness id (see nyx-agents harnesses)")?.to_string();
                let rest = args.iter().position(|a| a == "--").map(|i| args[i + 1..].to_vec()).unwrap_or_default();
                let cwd = opt(args, "--cwd")
                    .map(String::from)
                    .unwrap_or_else(|| std::env::current_dir().unwrap().display().to_string());
                let (cols, rows) = term_size().unwrap_or((120, 36));
                let s = c.launch(Launch {
                    harness,
                    cwd,
                    args: rest,
                    title: opt(args, "--title").map(String::from),
                    workspace: None,
                    // --worktree [NAME]: a new worktree, named or not.
                    worktree: args
                        .iter()
                        .position(|a| a == "--worktree")
                        .map(|i| args.get(i + 1).filter(|n| !n.starts_with("--")).cloned().unwrap_or_default()),
                    cols,
                    rows,
                })?;
                println!("{}", s.id);
            }
            "attach" => {
                let id = c.resolve(positional(args).ok_or("attach needs a session id")?)?;
                drop(c);
                return attach(&id, if flag(args, "--observe") { AttachMode::Observe } else { AttachMode::Control });
            }
            "resume" => {
                let id = c.resolve(positional(args).ok_or("resume needs a session id")?)?;
                println!("{}", c.resume(&id)?.id);
            }
            "stop" => {
                let id = c.resolve(positional(args).ok_or("stop needs a session id")?)?;
                c.stop(&id, flag(args, "--force"))?;
            }
            "forget" => {
                let id = c.resolve(positional(args).ok_or("forget needs a session id")?)?;
                c.forget(&id)?;
            }
            "hub" => {
                let path = nyx_agents_core::paths::hub_file();
                let print = || {
                    if let Ok(t) = std::fs::read_to_string(&path) {
                        println!("{t}");
                    }
                };
                print();
                if flag(args, "--watch") {
                    c.subscribe()?;
                    loop {
                        c.next_event()?;
                        // The manager rewrites the file before announcing a change.
                        print();
                    }
                }
            }
            other => {
                eprintln!("nyx-agents: unknown command {other}");
                return Ok(2);
            }
        }
        Ok(0)
    })();
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("nyx-agents: {e}");
            1
        }
    }
}

fn ago(ms: u64) -> String {
    let s = now_ms().saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn line(s: &Session) -> String {
    let state = match s.lifecycle {
        Lifecycle::Running => match s.status.activity {
            Activity::Blocked => "needs you".to_string(),
            Activity::Unknown => "running".to_string(),
            a => format!("{a:?}").to_lowercase(),
        },
        Lifecycle::Exited => format!("exited {}", s.exit_code.map(|c| c.to_string()).unwrap_or_default()),
        l => format!("{l:?}").to_lowercase(),
    };
    let branch = s.worktree.as_ref().and_then(|w| w.branch.as_deref()).map(|b| format!(" [{b}]")).unwrap_or_default();
    format!("{}  {:<8} {:<12} {:>4}  {}{}", &s.id[..8], s.harness, state, ago(s.status.since.max(s.created)), s.title, branch)
}

fn term_size() -> Option<(u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    (unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0).then_some((ws.ws_col, ws.ws_row))
}

struct RawMode(libc::termios);

impl RawMode {
    fn enter() -> Option<Self> {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(0, &mut t) } != 0 {
            return None;
        }
        let old = t;
        unsafe { libc::cfmakeraw(&mut t) };
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &t) };
        Some(Self(old))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

/// Attach the current terminal to a session. The screen comes from the
/// holder's snapshot, so reattaching shows exactly what the agent shows.
fn attach(id: &str, mode: AttachMode) -> Result<i32, Box<dyn std::error::Error>> {
    let mut a = Attachment::open(id, mode, None)?;
    let mut tx = a.sender()?;
    let size = term_size();
    if mode == AttachMode::Control
        && let Some((cols, rows)) = size
    {
        tx.resize(ResizeMsg { cols, rows, width: 0, height: 0 })?;
    }
    let raw = RawMode::enter();
    let mut out = std::io::stdout();
    {
        let mut tx = a.sender()?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut stdin = std::io::stdin();
            while let Ok(n) = stdin.read(&mut buf) {
                if n == 0 {
                    break;
                }
                // Ctrl+] detaches, leaving the agent running.
                if buf[..n].contains(&0x1d) {
                    unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
                    break;
                }
                if mode == AttachMode::Control && tx.input(&buf[..n]).is_err() {
                    break;
                }
            }
        });
    }
    unsafe { libc::signal(libc::SIGUSR1, on_detach as *const () as libc::sighandler_t) };
    let code = loop {
        let (kind, payload) = match a.next_frame() {
            Ok(f) => f,
            Err(_) => break 0,
        };
        match kind {
            Some(FrameKind::Snapshot) => {
                let t = Terminal::from_snapshot(&payload, &TerminalOptions::default())?;
                out.write_all(b"\x1b[H\x1b[2J\x1b[3J")?;
                out.write_all(&t.vt()?)?;
            }
            Some(FrameKind::Output) => out.write_all(&payload)?,
            Some(FrameKind::Exited) => {
                drop(raw);
                eprintln!("\r\n[session ended: {}]", String::from_utf8_lossy(&payload));
                return Ok(0);
            }
            _ => {}
        }
        out.flush()?;
    };
    drop(raw);
    Ok(code)
}

extern "C" fn on_detach(_: libc::c_int) {
    // Restoring the terminal happens in RawMode's drop on the normal path;
    // here the process is leaving, so restore and exit directly.
    let msg = b"\r\n[detached; the agent keeps running]\r\n";
    unsafe {
        let mut t: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut t) == 0 {
            t.c_lflag |= libc::ICANON | libc::ECHO | libc::ISIG;
            t.c_oflag |= libc::OPOST;
            t.c_iflag |= libc::ICRNL;
            libc::tcsetattr(0, libc::TCSANOW, &t);
        }
        libc::write(2, msg.as_ptr().cast(), msg.len());
        libc::_exit(0);
    }
}
