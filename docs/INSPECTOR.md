# Subagent Inspector

Under the agent's terminal, a row of tabs shows each subagent the agent
started, with its state (running, completed, failed). Selecting one opens a
smaller viewer below; the agent's terminal stays in view. Selecting it again,
or the arrow, hides the viewer.

## Read-only, always

The main agent alone manages its subagents. The inspector reads the
harness's own records and sends nothing anywhere: no input, prompts,
cancellation or reassignment, and it never writes to the records.

## Where the data comes from

| Harness | Source | Shown as |
|---|---|---|
| Claude Code | `~/.claude/projects/<cwd>/<session>/subagents/agent-<id>.jsonl` and `.meta.json`; the parent's `Agent` tool results | A transcript view (labelled; not a terminal) |
| Codex CLI | Rollouts under `~/.codex/sessions/YYYY/MM/DD/` whose `session_meta` names the parent in `source.subagent.thread_spawn.parent_thread_id`; `task_complete` / `turn_aborted` | A transcript view (labelled; not a terminal) |
| Others | Not integrated yet | No inspector |

The manager follows the records with inotify (no polling), publishes the
list on the session (`subagents`), and counts running ones in the AI Hub
feed. The app renders the selected transcript: the task, the agent's text,
each tool call with its main argument, and the first lines of each result.
Escape sequences and control characters in the records are dropped before
anything reaches the viewer.

A subagent counts as completed when the parent's transcript holds the
result of its `Agent` call, and as failed when that result is an error. A
background agent's call returns at once, so for those the subagent's own
transcript ending its turn marks completion.

## Deep links

`nyx-agents --open session/<id>/subagent/<agent id>` opens the session with
that subagent's viewer showing.

Codex compresses old rollouts (`.jsonl.zst`); live ones are plain, and only
those are followed.

## Tested

With a simulated Codex CLI writing rollouts in the format checked against
real ones on this machine (`docs/bench/inspector-codex.png`), and with a
simulated Claude Code that writes transcripts in the verified on-disk
layout (`docs/bench/inspector.png`). Real Claude Code subagents were not run
for this, to avoid spending tokens; the layout was checked against existing
real transcripts on this machine.
