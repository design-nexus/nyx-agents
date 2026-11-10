# Nyx Agents

A native NyxOS workspace for AI coding agents: Claude Code, Codex, oh-my-pi,
Muse, Grok Build, Antigravity, Crush, Cursor CLI, OpenCode and Hermes, each in
a Ghostty-based terminal, with a read-only inspector for the subagents they
start.

Status:

- **Phase 0 done.** The terminal (libghostty-vt 1.0 drawn by a GTK4 widget)
  works and is benchmarked; see [docs/TERMINAL.md](docs/TERMINAL.md).
- **Phase 1 done.** `nyx-agentd` owns sessions in holder processes that
  outlive the app and the manager; `nyx-agents` is its command line. Claude
  Code launches, runs and reattaches through it; the other nine harnesses
  are discovered and launch as plain terminal sessions.
- The desktop app comes next.

## Command line

```sh
nyx-agents harnesses             # what is installed, versions, integration tier
nyx-agents launch claude --cwd ~/Projects/foo
nyx-agents ls
nyx-agents attach 03c6           # Ctrl+] detaches; the agent keeps running
nyx-agents attach 03c6 --observe # read-only
nyx-agents stop 03c6 | resume 03c6 | forget 03c6
nyx-agents hub --watch           # the AI Hub feed
```

`tools/daemon-smoke.sh` checks the manager end to end with scratch folders
(`NYX_SMOKE_DBUS=1 dbus-run-session -- tools/daemon-smoke.sh` adds the D-Bus
actions on a private bus).

## Build

```sh
tools/fetch-libghostty-vt.sh      # about 10 MB, checked against SHA256SUMS
cargo build --release
./target/release/nyx-term-lab     # a shell in the prototype terminal
./target/release/nyx-term-lab --mirror
```

Needs Rust 1.95+, Zig 0.17, GTK 4.18+ and clang (for bindgen).

## Layout

- `crates/ghostty-vt-sys`: libghostty-vt built from the pinned release
- `crates/nyx-term`: safe terminal wrapper
- `crates/nyx-pty`: pseudo-terminals
- `crates/nyx-term-gtk`: the `TerminalView` widget
- `crates/nyx-agents-proto`: the manager's wire protocol
- `crates/nyx-agents-core`: harness adapters and discovery, records, git, hub
- `crates/nyx-agents-client`: client library
- `apps/nyx-agentd`: the manager and session holders
- `apps/nyx-agents`: the command line (and, next, the app)
- `apps/term-lab`: prototype window and benchmarks
- `docs/`: audit, terminal design and benchmark results
