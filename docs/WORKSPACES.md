# Agent workspaces

An agent workspace is a desktop of its own where an agent opens GUI programs
and drives them with its own pointer and keyboard (computer use), while you
keep working on yours. You can watch it live in Nyx Agents and take over.

Each workspace is a headless [sway](https://swaywm.org) run by `nyx-agentd`.
It never appears on your screen and never touches your Hyprland session.

```
harness (Claude Code, Codex) --MCP stdio--> nyx-agents computer --socket--> nyx-agentd
nyx-agentd --spawn, i3 IPC, virtual input, screencopy--> headless sway --> the agent's programs
Nyx Agents app --screencopy (and virtual input when you take over)--> headless sway
```

## Using it

```sh
nyx-agents launch claude --cwd ~/Projects/foo --workspace new
nyx-agents workspaces                 # id, size, windows, sessions, focused window
nyx-agents --open workspace/ID        # the live view
nyx-agents workspaces new --size 1440x900 --name Research
nyx-agents launch codex --workspace ID
nyx-agents workspaces rm ID
```

A session started with `--workspace` (`Launch.workspace` on the socket:
`"new"` or an id) gets the `nyx-computer` MCP server: Claude Code through
`--mcp-config`, Codex through `-c mcp_servers.nyx_computer.*`. The other
harnesses are not wired yet; they can run
`nyx-agents computer --workspace ID` themselves as a stdio MCP server.
Sessions also get `NYX_AGENT_WORKSPACE` and `NYX_AGENT_WAYLAND_DISPLAY`
(the workspace's socket, an absolute path) in their environment.

Tools: `screenshot`, `click`, `move`, `drag`, `scroll`, `type`, `key`,
`open_app` (an argv, never a shell string), `list_windows`, `focus_window`,
`resize_screen`. Action tools take `screenshot: true` to return a capture
taken just after. Coordinates are pixels in the screenshot: workspaces are
1280×800 at scale 1 by default, and screenshots are always at logical size.

In the app, a session in a workspace has a display button in its bar that
opens the live view. The view redraws when the screen changes, at most about
15 times a second. **Take over** (off by default) sends your clicks, scrolls
and keys into the workspace; the agent can still act while you do.

## Protocol

`workspaces.list`, `.get`, `.create`, `.remove`, `.resize`, `.launch`,
`.windows`, `.focus_window`, `.screenshot` and `.input` on the manager's
socket (see `crates/nyx-agents-proto`). `AgentWorkspace.compositor` is
`sway:<socket path>`. `workspace` and `workspace-removed` events arrive on
the subscribe stream. The D-Bus control interface adds
`agents.list_workspaces` (read only) and `agents.open_workspace`.

The backend sits behind `WorkspaceBackend` in
`apps/nyx-agentd/src/workspaces.rs`; a compositor-native backend could
replace sway there without changing the protocol.

## Lifetime

Workspaces belong to the manager and end with it: if `nyx-agentd` restarts,
its workspaces close (their sessions keep running, and their tools report
that the workspace ended). One with no running session is removed after ten
minutes without use. Removing a workspace ends every program in it and
deletes its folders.

## Safety

- sway starts with `WLR_BACKENDS=headless`, `WLR_RENDERER=pixman` and
  `WLR_LIBINPUT_NO_DEVICES=1`: no DRM device, no login session, no GPU.
- Its environment is rebuilt from an allow-list. `WAYLAND_DISPLAY`,
  `DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK` and the D-Bus address
  are never passed on.
- It gets a generated config (`--config`): no bar, no background program, no
  key bindings, no Xwayland, no swaynag. Your sway config is never read.
- A watchdog reads sway's log and kills it at the first line showing a DRM,
  session, libinput, Wayland or X11 backend, a GPU renderer or Xwayland. The
  manager also refuses a sway whose outputs are not all `HEADLESS-*`.
- Each workspace has its own runtime folder as `XDG_RUNTIME_DIR`
  (`$XDG_RUNTIME_DIR/nyx-agents/ws/<id>/run`). A program inside that looks
  for a display finds only the workspace's, and can't reach your Hyprland
  socket or `hyprctl`.
- Each has a private D-Bus session bus, so single-instance GTK apps register
  there instead of handing their window to a copy on your desktop.
- Chromium-family browsers, Firefox-family browsers and VS Code-family
  editors get a profile in the workspace (`~/.local/state/nyx-agents/workspaces/<id>/profiles`),
  unless the argv already names one, so they don't hand off to a running
  copy either.
- sway and its programs are spawned with `PR_SET_PDEATHSIG` from one
  long-lived thread, so they end with the manager.

## Limits

- **Not a security boundary.** Programs in a workspace run as you and can
  read and change your files, use your network, and reach anything not
  listed above.
- Programs the agent starts from its own shell (not through `open_app`)
  still use your display and D-Bus. Point them at the workspace with
  `WAYLAND_DISPLAY=$NYX_AGENT_WAYLAND_DISPLAY`.
- No Xwayland: X11-only programs don't run.
- No clipboard or file-picker bridge between a workspace and your desktop,
  no audio, no portals.
- Programs started some other way that hand off through a socket of their
  own (an Electron app's single-instance lock, for example) may still open
  on your desktop.

## Testing

Unit tests (`cargo test -p nyx-agent-workspace -p nyx-agentd -p nyx-agents-proto`)
cover the config, the environment, the watchdog, IPC encoding, keymaps,
capture conversion, PNG and the MCP schemas. The integration test starts a
real headless sway and needs sway and foot installed:

```sh
cargo test -p nyx-agent-workspace --features headless-it -- --test-threads=1
```

It records your Hyprland focus and pointer before and after and fails if
either changed.
