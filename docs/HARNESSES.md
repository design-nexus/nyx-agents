# Harnesses

What Nyx Agents does with each approved harness, as verified on this machine
on 2026-11-10. Anything not listed as supported is hidden in the app.

| Harness | Version | Tier | Launch | Status (OSC 7501) | Resume | Subagents |
|---|---|---|---|---|---|---|
| Claude Code (`claude`) | 2.1.322 | native | yes, with a preset session id | not seen yet | yes, once a conversation exists | yes: transcripts, read-only |
| Codex CLI (`codex`) | 0.167.0 | native | yes | not seen | yes: id read from Codex's rollout (`codex resume <id>`) | yes: child rollouts, read-only |
| Grok Build (`grok`) | 1.0.77 | native | yes, with a preset session id (`--session-id`) | yes | yes (`--resume <id>`) | not yet |
| Antigravity CLI (`agy`) | 1.5.4 | terminal | yes | yes | not yet | not yet |
| oh-my-pi (`omp`) | 18.x | native | yes | yes | yes: id read from its session file (`-r <id>`) | not yet |
| Crush (`crush`) | 0.99 / 0.100 | terminal | yes | yes | not yet | not yet |
| OpenCode (`opencode`) | 2.2.6 | terminal | yes | yes | not yet | not yet |
| Muse Code (`muse`) | 1.4.14 | terminal | yes | not seen | not yet | not yet |
| Cursor CLI (`cursor-agent`) | 2026.11.05 | native | yes | not seen | yes, once the chat has a conversation: id from its `meta.json` (`--resume <chatId>`) | not yet |
| Hermes (`hermes`) | 0.21.4 | terminal | yes (local) | not seen | not yet | not yet |
| Terminal (`$SHELL`) | | terminal | yes | through the shell | no | no |
| Generic (`harnesses.toml`) | | generic | yes | if the command reports it | no | no |

**Launch** was tested by starting each real harness through the manager in a
scratch folder and reading its first screen, without typing anything.
**Status** is "yes" where the harness was seen reporting OSC 7501.
**Resume** for Codex, Cursor and oh-my-pi was tested with simulated
harnesses writing session records in the layouts checked on this machine; Grok's flags were checked against `grok --help`; Claude's
was tested for real.

Not yet integrated, with what is known:

- **Antigravity**: `--conversation <id>` resumes; ids live in SQLite with protobuf payloads.
- **OpenCode**: `-s <id>` continues a session; the id format it accepts for a new one is unverified.
- **Muse**: only non-interactive runs take a session id; `muse resume <ref>` exists.
- **Crush**: `-s <id>`; ids from `crush session list --json`.
- **Hermes**: `-r <id>`; remote use (`hermes serve`) is not integrated.

Discovery never runs mise shims or command wrappers (they can install tools).
`/usr/bin/antigravity` is the Antigravity IDE and is not used.
