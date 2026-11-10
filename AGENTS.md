# Nyx Agents

Native GTK4 app and user daemon for running AI coding agents in Ghostty-based
terminals. Crates live in `crates/`, programs in `apps/`.

Commands: `tools/fetch-libghostty-vt.sh` (once), `cargo build --release`,
`cargo test -p nyx-term -p nyx-pty -p nyx-term-gtk`.

- libghostty-vt is pinned in `vendor/libghostty-vt/SHA256SUMS` and builds
  offline with Zig 0.17. Never substitute VTE or another emulator.
- Programs are spawned from an argv, never through a shell string.
- Never execute mise shims to discover harnesses; they install tools.
- Subagent views are read-only: no input, no resize, no query replies.
- Test GTK code on a private Xvfb display or headless output with scratch XDG
  folders and an `*_APP_ID` override; never on the user's screen.

Use the public, pinned shared crates; never import another application's internals.
Keep application names, IDs, paths, D-Bus interfaces, and data formats stable.
Common guidance: https://github.com/design-nexus/nyx/blob/main/docs/AGENTS_COMMON.md
Never force-push, delete history, or publish a new GitHub repository without
explicit approval.
