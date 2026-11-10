# AI Hub feed

Nyx Agents publishes its sessions for the AI Hub (and any other shell
widget) without anything polling.

## The snapshot file

`$XDG_RUNTIME_DIR/nyx-agents/hub.json` (mode 0600), rewritten atomically by
`nyx-agentd` on every change. Watch it; don't poll.

```json
{
  "version": 1,
  "updated": 1794290000000,
  "agents": 2, "blocked": 1, "done": 0, "working": 1, "subagents_running": 0,
  "sessions": [
    {
      "id": "03c6123568cb717dfe390da547bcfccf",
      "harness": "claude",
      "title": "Claude Code · nyx-agents",
      "project": "9f2c41d07a1b33e8",
      "cwd": "/home/ken/Projects/nyx-agents",
      "branch": "main",
      "activity": "blocked",
      "blocked_on": "permission",
      "turn": 3,
      "started": 1794289000000,
      "status_since": 1794289990000,
      "subagents_running": 0,
      "subagents_total": 0,
      "workspace": null,
      "open": ["nyx-agents", "--open", "session/03c6123568cb717dfe390da547bcfccf"]
    }
  ]
}
```

- Only live sessions (starting or running) are listed; the counts match the
  list. `agents`, `blocked`, `done` and `working` are what the current Live
  tab's bar dot uses.
- `activity`: `unknown` (the harness reports nothing), `idle`, `working`,
  `blocked`, `done`, `error`. `blocked_on`: `permission`, `question`, `auth`
  or null.
- `turn` increases each time the agent stops working. A client that stores
  the last turn it showed can tell "finished while you weren't looking"
  (`done` and `turn` greater than the stored one) without writing anything
  back.
- `open` is an argv. Run it as given (never through a shell) to show the
  session in Nyx Agents.
- Titles come from users and agents and are already stripped of control and
  invisible characters; treat them as plain text.
- Not included: process ids, harness session ids, environment, prompts or
  output.

## Compatibility

`version` changes only for incompatible changes. New fields may appear in
version 1 at any time; ignore what you don't know.

## Stream instead of a file

`nyx-agents hub --watch` prints the current snapshot as one JSON line, then
one line after every change, until killed.

## The hub only monitors

Stopping, resuming or sending input to agents happens in Nyx Agents, not in
the hub. Deep links are the only action the feed offers.

## QML

`qml/NyxAgentsService.qml` is a ready-made Quickshell service: it watches
the file and exposes `running`, the counts, and `sessions`, plus
`open(session)`.

## Glimpse

`apps/agents-glimpse` is a Glimpse plugin over the same file. Glimpse mounts
only `hub.json` into its sandbox (as `$NYX_AGENTS_HUB`), never the runtime
folder's sockets, and lets the plugin open only `nyx-agents://` links, which
Nyx Agents handles (`nyx-agents://session/<id>[/subagent/<id>]`). Install
installs it to `~/.local/share/nyx-agents/glimpse`; add it to Glimpse with
`nyx-glimpse plugins install-local ~/.local/share/nyx-agents/glimpse` and
enable it in Glimpse's plugin settings.
