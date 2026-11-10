# Agent workspaces

An agent workspace is a desktop of its own where an agent opens GUI programs
and drives them with its own pointer and keyboard (computer use), while you
keep working on yours. You can watch it live in Nyx Agents and take over.

Each workspace is a headless [sway](https://swaywm.org) run under a holder
process of its own. It never appears on your screen and never touches your
Hyprland session.

```
harness --MCP stdio--> nyx-agents computer --socket--> nyx-agentd
nyx-agentd --holder.sock: launch, resize, clipboard, stop--> nyx-agentd ws-hold --> headless sway --> programs
nyx-agentd --i3 IPC, virtual input, screencopy--> headless sway
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

New session in the app has **With a desktop of its own** for the same thing.

### Which harnesses get the tools

- **Claude Code, Codex and Muse** get the `nyx-computer` MCP server for each
  session with a workspace, on their command line (`--mcp-config`,
  `-c mcp_servers.nyx_computer.*`, `-c mcpServers.nyx-computer=…`).
- **Grok, Hermes, Antigravity, Cursor CLI, oh-my-pi, OpenCode and Crush**
  only read MCP servers from their config. `nyx-agents computer register`
  adds one `nyx-computer` entry to each that is installed (through its own
  `mcp add` where it has one; otherwise one key in its user config, keeping
  the rest). That server finds the workspace of the session it runs under
  and offers no tools in sessions without one. `--dry-run` lists what would
  change; `--remove` takes the entries out again.

Sessions in a workspace also get `NYX_AGENT_WORKSPACE`, and their
`WAYLAND_DISPLAY` and `DISPLAY` point at the workspace, so GUI programs the
agent starts from its own shell open there too.

### Tools

`screenshot`, `click`, `move`, `drag`, `scroll`, `type`, `key`, `open_app`
(an argv, never a shell string), `list_windows`, `focus_window`,
`resize_screen`, `get_clipboard` and `set_clipboard`. Action tools take
`screenshot: true` to return a capture taken just after. Coordinates are
pixels in the screenshot: workspaces are 1280×800 at scale 1 by default, and
screenshots are always at logical size. The keyboard is a US layout, so keys
reach programs on their real keycodes; characters a US keyboard lacks are
typed through spare keycodes.

### In the app

A session in a workspace has a display button in its bar that opens the live
view. The view redraws when the screen changes, at most about 15 times a
second. **Take over** (off by default) sends your clicks, scrolls and keys
into the workspace and puts your clipboard there; the agent can still act
while you do. Right-click the view to copy either clipboard to the other.

Because an agent in a workspace uses the workspace's clipboard, pasting an
image into its terminal (Ctrl+V with only an image copied) first puts the
image on the workspace's clipboard, so Claude Code and Codex still find it.

### What runs inside

- Wayland programs, and X11 programs through Xwayland (started the first
  time one connects).
- Programs that draw with OpenGL, on Mesa's software renderer: Ghostty and
  GTK apps work. GTK is told to skip Vulkan; there is no software Vulkan
  driver unless lavapipe is installed.
- Sound plays through your PipeWire and PulseAudio.

## Protocol

`workspaces.list`, `.get`, `.create`, `.remove`, `.resize`, `.launch`,
`.windows`, `.focus_window`, `.screenshot`, `.input`, `.clipboard_get` and
`.clipboard_set` on the manager's socket (see `crates/nyx-agents-proto`).
`AgentWorkspace.compositor` is `sway:<socket path>`. `workspace` and
`workspace-removed` events arrive on the subscribe stream. The D-Bus control
interface adds `agents.list_workspaces` (read only) and
`agents.open_workspace`.

The backend sits behind `WorkspaceBackend` in
`apps/nyx-agentd/src/workspaces.rs`; a compositor-native backend could
replace sway there without changing the protocol.

## Lifetime

Each workspace has a holder (`nyx-agentd ws-hold`) in a session of its own,
like an agent's terminal, so it keeps running when the manager restarts or
is updated; the manager finds it again from `holder.json` and its socket. It
ends when it is removed, when its sway ends, or at logout. One with no
running session is removed after ten minutes without use. Removing a
workspace ends every program in it and deletes its folders.

## Safety

- sway starts with `WLR_BACKENDS=headless`, `WLR_RENDERER=pixman` and
  `WLR_LIBINPUT_NO_DEVICES=1`: no DRM device, no login session, no GPU.
- Its environment is rebuilt from an allow-list. `WAYLAND_DISPLAY`,
  `DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE`, `SWAYSOCK` and the D-Bus address
  are never passed on.
- It gets a generated config (`--config`): no bar, no background program, no
  key bindings, no swaynag. Your sway config is never read. Its one command
  writes its X display to the workspace's folder.
- A watchdog reads sway's log and kills it at the first line showing a DRM,
  session, libinput, Wayland or X11 backend or a GPU renderer. The manager
  also refuses a sway whose outputs are not all `HEADLESS-*`.
- Each workspace has its own runtime folder as `XDG_RUNTIME_DIR`
  (`$XDG_RUNTIME_DIR/nyx-agents/ws/<id>/run`). A program inside that looks
  for a display finds only the workspace's, and can't reach your Hyprland
  socket or `hyprctl`.
- Each has a private D-Bus session bus that starts nothing on demand, so
  single-instance GTK apps register there instead of handing their window to
  a copy on your desktop, and no portal or gvfs mounts appear inside it.
- Removing a workspace unmounts anything mounted inside its folder and never
  deletes across a mount point or through a symlink.
- Chromium-family browsers, Firefox-family browsers and VS Code-family
  editors get a profile in the workspace (`~/.local/state/nyx-agents/workspaces/<id>/profiles`),
  unless the argv already names one, so they don't hand off to a running
  copy either.
- sway's binary carries file capabilities (`cap_sys_nice`), which clears the
  parent-death signal, so a guard forked beside it kills its process group
  if the holder ends for any reason. Programs started in the workspace get
  the parent-death signal from the holder.

## Limits

- **Not a security boundary.** Programs in a workspace run as you and can
  read and change your files, use your network, and reach anything not
  listed above.
- The agent's own shell keeps your D-Bus bus (for keyrings and the like), so
  a single-instance GTK app it starts while a copy runs on your desktop may
  still open there.
- No portals and no on-demand D-Bus services in a workspace: apps use GTK's
  own file chooser, and settings they save through dconf aren't kept.
- Workspaces don't survive a logout or reboot; the sessions' records do.

## Testing

Unit tests (`cargo test -p nyx-agent-workspace -p nyx-agentd -p nyx-agents-proto`)
cover the config, the environment, the watchdog, the bus config, removal,
IPC encoding, keymaps, capture conversion, PNG, base64 and the MCP schemas.
The integration test starts a real headless sway and needs sway and foot:

```sh
cargo test -p nyx-agent-workspace --features headless-it -- --test-threads=1
```

It types a line of punctuation and accented text into `cat` in foot and
checks the file, passes text through the clipboard between two clients,
checks the X display, captures and resizes. It fails if your Hyprland focus
changes or a window appears on your desktop.
