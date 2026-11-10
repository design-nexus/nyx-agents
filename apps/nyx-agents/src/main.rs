//! nyx-agents: the desktop app, and its command line.
//!
//!   nyx-agents [--open session/ID]       the window (ID may be a prefix; also workspace/ID)
//!   nyx-agents harnesses [--json] [--refresh]
//!   nyx-agents ls [--json]
//!   nyx-agents status ID [--json]
//!   nyx-agents launch HARNESS [--cwd DIR] [--title T] [--worktree [NAME]] [--workspace new|ID] [-- ARGS...]
//!   nyx-agents attach ID [--observe]     Ctrl+] detaches
//!   nyx-agents resume ID | stop ID [--force] | forget ID
//!   nyx-agents workspaces [--json] | workspaces new [--name N] [--size WxH] | workspaces rm ID
//!   nyx-agents hub [--watch]
//!   nyx-agents usage [AGENT] [--json] [--refresh] [--local]   --local scans here, without the manager
//!   nyx-agents settings ...            the Nyx Settings page
//!   nyx-agents computer --workspace ID computer-use tools (an MCP server on stdio)
//!
//! IDs may be shortened to any unique prefix.

mod cli;
mod computer;
mod settings;
mod ui;

const COMMANDS: &[&str] =
    &["harnesses", "ls", "status", "launch", "attach", "resume", "stop", "forget", "workspaces", "hub", "usage", "help", "-h", "--help"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "computer") {
        std::process::exit(computer::run(&args[1..]));
    }
    if args.first().is_some_and(|a| a == "settings") {
        std::process::exit(settings::run(&args[1..]));
    }
    if args.first().is_some_and(|a| COMMANDS.contains(&a.as_str())) {
        std::process::exit(cli::run(&args));
    }
    std::process::exit(ui::run());
}
