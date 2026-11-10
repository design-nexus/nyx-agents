# Nyx Agents

A native NyxOS workspace for AI coding agents: Claude Code, Codex, oh-my-pi,
Muse, Grok Build, Antigravity, Crush, Cursor CLI, OpenCode and Hermes, each in
a Ghostty-based terminal, with a read-only inspector for the subagents they
start.

Status: **Phase 0 done.** The terminal (libghostty-vt 1.0 drawn by a GTK4
widget) works and is benchmarked; see [docs/TERMINAL.md](docs/TERMINAL.md).
The session daemon and the app come next.

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
- `apps/term-lab`: prototype window and benchmarks
- `docs/`: audit, terminal design and benchmark results
