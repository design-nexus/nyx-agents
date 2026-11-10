//! nyx-agents: the command line (the desktop app comes in Phase 2).
//!
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(cli::run(&args));
}
