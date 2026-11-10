# Audit (2026-11-09)

Confirmed findings that shape Nyx Agents. Paths are under `~/Projects`.

## Nyx infrastructure to reuse

- **No Nyx Agents code existed.** The Hyprland workspace-name prefix
  `nyx-agent` is already reserved and hidden by the switcher, dock and
  desktop (`nyx-switcher/crates/nyx-switcher-core/src/model.rs`,
  `nyx-core/crates/nyx-desktop/src/model.rs`). Agent workspaces themselves are
  being built into Hyprland in a separate effort; Nyx Agents will use that API
  when it lands.
- **Design 2.0**: `nyx_theme::use_design_2()` + `nyx_theme::install(Style)`,
  widgets from `nyx-design/crates/nyx-ui`, rules in `nyx-design/STYLE.md`.
  Theme Engine 2.0 is `io.github.design_nexus.nyx.Theme1`; Catppuccin Mocha
  is the NyxOS default. The engine's palette has no ANSI colours, so the
  terminal reads Omarchy's `colors.toml` and falls back to Catppuccin Mocha.
- **App template**: `nyx-containers` (app-id env override,
  `HANDLES_COMMAND_LINE`, pinned `nyx-core`/`nyx-ui`/`nyx-theme` revisions).
  It embeds VTE; Nyx Agents does not.
- **Daemon template**: `nyx-ai` (`nyx-aid` socket activation, NDJSON over
  `$XDG_RUNTIME_DIR`, `install.sh` installs user units).
- **Shared action registry**: apps expose `io.github.design_nexus.nyx.Control`
  (`Describe`/`Call`, served by `nyx_core::control::serve`); `nyx-aid`
  discovers them. Nyx CLI actions go there.
- **Glimpse**: `plugin.toml` + `glimpse-plugin-sdk`. **Settings**:
  `extension.toml` + a `nyx-<app>-settings` helper. **Notifications**:
  `gio::Notification`/FDO, recorded by `nyx-notifyd`. **Nyx Git**:
  `nyx-git <path>` and Control `repo.open`.

## AI Hub

`omarchy-ai-hub` is an Omarchy bar plugin (QML) whose pages run Python/bash
scanners on timers; there is no daemon. Its Live tab shows herdr sessions. A
new AI Hub plugin is planned; Nyx Agents publishes a live session feed for it
(`docs/HUB.md`, Phase 2b) and leaves the current plugin untouched.

## Ghostty and Herdr

- libghostty-vt 1.0: emulation only, stable C ABI, needs Zig 0.17 to build.
  See `TERMINAL.md`.
- Herdr TUI, Herdr Desktop (`herdrdev/herdr-app`, GPUI) and herdr-core (`hc`)
  are Apache-2.0 Rust. Desktop is a client of herdr-core, which owns PTYs in
  holder processes, takes status only from OSC 7501, and does not resume
  agents after a reboot. Adopted ideas: daemon-owned PTYs with reattach,
  OSC 7501 status with a "seen" watermark, the Needs you / Done / Working /
  Idle list, subagents nested under their parent, worktree new/open/remove.
  Not adopted: QuickJS mods, layout DSL, telemetry, auto-update, interactive
  peek into children, children dying with the UI.

## Harnesses (versions verified on this machine)

| Harness | Version | Structured interface | Subagent data |
|---|---|---|---|
| Claude Code | 2.1.322 | `--session-id`, `-r`, stream-json, hooks | `<session>/subagents/agent-*.jsonl` + `.meta.json` (transcripts, not PTYs) |
| Codex CLI | 0.167.0 | `exec --json`, `app-server`, hooks | rollout `session_meta.source.subagent.thread_spawn` |
| Antigravity (Google `agy`) | 1.5.3 | stream-json, `acp` | SQLite `parent_conversation_id` (protobuf payloads) |
| oh-my-pi (`omp`) | 18.18.6 | `--mode rpc|json`, `acp` | jsonl in the session folder |
| Muse (Meta Muse Code) | 1.4.14 | `exec --json`, `serve` (MSP) | `<session>/subagent/<id>/session.jsonl` |
| Grok Build (xAI `grok`) | 1.0.77 | streaming-json, `agent stdio|serve` | not seen on disk |
| Crush | 0.99.0 | `server` (HTTP) | `parent_session_id` column, no rows |
| Cursor CLI | 2026.11.05 | stream-json, `--resume` | transcript files (may be IDE-written) |
| OpenCode | 2.2.6 | `serve`, `run --format json` | `session.parent_id`, no rows |
| Hermes (Nous) | 0.21.4 | `serve` (WebSocket), `acp` | `parent_session_id`, no rows |

Gemini CLI and Pi Forge are out of scope. Discovery must never execute mise
shims: running `omp --version` through a shim installed oh-my-pi during the
audit.

## Risks

- **Compositor stability during tests**: Hyprland crashed once during
  headless-output benchmarks (see `TERMINAL.md`). Tests now use Xvfb.
- **Subagent visibility differs per harness**: Claude Code subagents have no
  PTY, so the inspector shows their transcripts, labelled as such.
- **Agent survival**: agents outlive the app and a daemon restart (holder
  processes), not a reboot. After a reboot sessions are offered for resume.
