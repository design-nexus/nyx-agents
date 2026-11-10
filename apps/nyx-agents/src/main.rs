//! nyx-agents: the desktop app, and its command line.
//!
//!   nyx-agents [--open session/ID]       the window (ID may be a prefix)
//!   nyx-agents harnesses [--json] [--refresh]
//!   nyx-agents ls [--json]
//!   nyx-agents status ID [--json]
//!   nyx-agents launch HARNESS [--cwd DIR] [--title T] [-- ARGS...]
//!   nyx-agents attach ID [--observe]     Ctrl+] detaches
//!   nyx-agents resume ID | stop ID [--force] | forget ID
//!   nyx-agents hub [--watch]
//!
//! IDs may be shortened to any unique prefix.

mod cli;
mod ui;

const COMMANDS: &[&str] =
    &["harnesses", "ls", "status", "launch", "attach", "resume", "stop", "forget", "hub", "help", "-h", "--help"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| COMMANDS.contains(&a.as_str())) {
        std::process::exit(cli::run(&args));
    }
    std::process::exit(ui::run());
}
