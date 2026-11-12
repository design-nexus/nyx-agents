//! Nyx Agent Manager.
//!
//!   nyx-agentd [serve]                      the manager (systemd starts it on first use)
//!   nyx-agentd hold --id ID [--cols N --rows N] --cwd DIR -- ARGV...
//!                                           one session's holder (started by the manager)
//!   nyx-agentd ws-hold --id ID [--name N --width W --height H --scale S]
//!                                           one agent workspace's holder (started by the manager)
//!
//! Holders own each agent's pty in their own process and session, so agents
//! keep running when the app closes and when the manager restarts; the
//! manager reattaches to them. They do not survive a reboot: their records
//! and terminal history do, and sessions whose harness supports it can be
//! resumed.

mod control;
mod holder;
mod notify;
mod peer;
mod server;
mod workspaces;
mod ws_holder;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        None | Some("serve") => server::run(),
        Some("hold") => holder::run(&args[2..]),
        Some("ws-hold") => ws_holder::run(&args[2..]),
        Some("-h" | "--help") => {
            println!("usage: nyx-agentd [serve] | nyx-agentd hold --id ID --cwd DIR -- ARGV...");
            0
        }
        Some(other) => {
            eprintln!("nyx-agentd: unknown command {other}");
            2
        }
    };
    std::process::exit(code);
}

/// A command running this program, for starting holders. `/proc/self/exe`
/// still runs after an install replaces the file; the path `current_exe`
/// gives then does not. The installed path stays the command's name, so
/// `ps` and `pgrep` still know a holder.
pub fn self_command() -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let path = std::env::current_exe().unwrap_or_else(|_| "nyx-agentd".into());
    let name = path.to_string_lossy().trim_end_matches(" (deleted)").to_string();
    let proc = std::path::Path::new("/proc/self/exe");
    let mut cmd = std::process::Command::new(if proc.exists() { proc } else { path.as_path() });
    cmd.arg0(name);
    cmd
}

/// Append a line to a log file, keeping it under 1 MB.
pub fn log_to(path: &std::path::Path, line: &str) {
    use std::io::Write;
    if path.metadata().map(|m| m.len() > 1 << 20).unwrap_or(false) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {line}", nyx_agents_proto::now_ms());
    }
}
