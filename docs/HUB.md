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
  ],
  "usage": [
    {
      "harness": "claude", "name": "Claude Code", "working": true,
      "last_used": 1794290000, "today_prompts": 38, "today_tokens": 1100000,
      "quota": { "id": "claude-five_hour", "label": "5 hours", "used_percent": 4.0,
                 "resets_at": 1794306000, "burn_per_hour": 0.5, "pace": "on-pace",
                 "left_at_reset": 88.0 }
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
- `usage` is each agent with records or a quota, working first, then last
  used first. `quota` is the window closest to running out, only when quota
  checks are on for that agent (see Usage below). `pace`: `steady`,
  `on-pace`, `tight` (under 15% left at reset), `runs-out` (with `empty_in`
  seconds), `stale` (reset since it was fetched).

## Usage

`usage.list {refresh}` and `usage.get {harness, refresh}` on the control
socket return each agent's full usage: today's prompts, steps, tokens and
cached tokens, the last seven days, models (today, week, all), the top tools,
the ten most recent sessions, and the quota. A `usage` event follows every
change. `nyx-agents usage [AGENT] [--json] [--refresh]` prints it, and
`--local` scans without the manager.

- Read from the agents' own records: Claude Code's transcripts and Codex's
  rollouts. Reads are incremental (a byte-offset cache per record in
  `~/.cache/nyx-agents/usage`), every minute, and every ten seconds while the
  agent is working.
- Recent sessions carry the session's first prompt as its title, so the full
  usage stays on the control socket (same user only) and out of `hub.json`.
- Quotas are off until you turn them on in Nyx Settings (Agents, Quotas), which
  lists the agents in `~/.config/nyx-agents/config.toml`:

  ```toml
  [usage]
  quotas = ["claude", "codex"]
  ```

  Claude's comes from Anthropic's OAuth usage endpoint with the token Claude
  Code keeps in `~/.claude/.credentials.json`; Codex's from `codex app-server`
  (read-only sandbox, no model calls). Tokens are read in `nyx-agentd` only,
  sent only to their provider, and never stored, logged or passed on. After a
  failure a provider is asked again after five minutes, or on refresh.

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
