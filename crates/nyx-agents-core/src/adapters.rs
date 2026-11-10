//! Harness adapters. Each advertises only what is implemented for it; the
//! rest of the app hides anything a harness does not support.

use std::path::{Path, PathBuf};

use nyx_agents_proto::{AuthState, Capabilities, Harness, Session, Tier};
use serde::Deserialize;

use crate::discover;

/// What to run for a session.
#[derive(Clone, Debug)]
pub struct Plan {
    pub argv: Vec<String>,
    /// The harness's own session id, when chosen up front.
    pub harness_session: Option<String>,
}

struct Builtin {
    id: &'static str,
    name: &'static str,
    exe: &'static [&'static str],
    mise: &'static [&'static str],
    /// Files whose presence means credentials are set up (never read).
    auth_files: &'static [&'static str],
    auth_env: &'static [&'static str],
    tier: Tier,
}

/// The approved harnesses. Gemini CLI and Pi Forge are deliberately absent.
const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "claude",
        name: "Claude Code",
        exe: &["claude"],
        mise: &["claude"],
        auth_files: &[".claude/.credentials.json"],
        auth_env: &["ANTHROPIC_API_KEY"],
        tier: Tier::Native,
    },
    Builtin {
        id: "codex",
        name: "Codex CLI",
        exe: &["codex"],
        mise: &["codex"],
        auth_files: &[".codex/auth.json"],
        auth_env: &["OPENAI_API_KEY"],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "antigravity",
        name: "Antigravity CLI",
        // Only the CLI: /usr/bin/antigravity is the Antigravity IDE.
        exe: &["agy"],
        mise: &["antigravity-cli"],
        auth_files: &[],
        auth_env: &[],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "omp",
        name: "oh-my-pi",
        exe: &["omp"],
        mise: &["oh-my-pi"],
        auth_files: &[],
        auth_env: &[],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "muse",
        name: "Muse Code",
        exe: &["muse"],
        mise: &["muse"],
        auth_files: &[],
        auth_env: &[],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "grok",
        name: "Grok Build",
        exe: &["grok"],
        mise: &["grok"],
        auth_files: &[".grok/auth.json"],
        auth_env: &["XAI_API_KEY"],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "crush",
        name: "Crush",
        exe: &["crush"],
        mise: &["crush", "github-charmbracelet-crush"],
        auth_files: &[],
        auth_env: &[],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "cursor",
        name: "Cursor CLI",
        exe: &["cursor-agent"],
        mise: &["cursor-agent"],
        auth_files: &[],
        auth_env: &["CURSOR_API_KEY"],
        tier: Tier::Terminal,
    },
    Builtin {
        id: "opencode",
        name: "OpenCode",
        exe: &["opencode"],
        mise: &["opencode"],
        auth_files: &[".local/share/opencode/auth.json"],
        auth_env: &[],
        tier: Tier::Terminal,
    },
    Builtin { id: "hermes", name: "Hermes", exe: &["hermes"], mise: &[], auth_files: &[], auth_env: &[], tier: Tier::Terminal },
];

/// A command from `~/.config/nyx-agents/harnesses.toml`:
///
/// ```toml
/// [[harness]]
/// id = "aider"
/// name = "Aider"
/// command = ["aider", "--no-auto-commits"]
/// ```
#[derive(Clone, Debug, Deserialize)]
pub struct GenericDef {
    pub id: String,
    pub name: String,
    pub command: Vec<String>,
}

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    harness: Vec<GenericDef>,
}

pub fn generic_defs() -> Vec<GenericDef> {
    let path = crate::paths::config_dir().join("harnesses.toml");
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    toml::from_str::<Config>(&text)
        .map(|c| c.harness)
        .unwrap_or_default()
        .into_iter()
        .filter(|g| !g.command.is_empty() && g.id != "shell" && !BUILTINS.iter().any(|b| b.id == g.id))
        .collect()
}

fn auth(b: &Builtin) -> AuthState {
    if b.auth_files.is_empty() && b.auth_env.is_empty() {
        return AuthState::Unknown;
    }
    let home = crate::paths::home();
    let ready = b.auth_files.iter().any(|f| home.join(f).exists()) || b.auth_env.iter().any(|v| std::env::var_os(v).is_some());
    match (ready, b.auth_files.is_empty()) {
        (true, _) => AuthState::Ready,
        // Without a known credentials file, a missing variable proves nothing.
        (false, true) => AuthState::Unknown,
        (false, false) => AuthState::NotReady,
    }
}

fn caps(id: &str) -> Capabilities {
    match id {
        "claude" => Capabilities {
            preset_session_id: true,
            resume: true,
            subagents: Some(nyx_agents_proto::SubagentSource::Transcript),
            ..Default::default()
        },
        _ => Capabilities::default(),
    }
}

/// Every known harness, installed or not. `versions` runs `--version` on
/// installed ones (slow: call off the main thread and cache).
pub fn harnesses(versions: bool) -> Vec<Harness> {
    let mut out: Vec<Harness> = BUILTINS
        .iter()
        .map(|b| {
            let path = discover::find(b.exe, b.mise);
            Harness {
                id: b.id.into(),
                name: b.name.into(),
                tier: b.tier,
                version: path.as_deref().filter(|_| versions).and_then(discover::version),
                path: path.map(|p| p.display().to_string()),
                auth: auth(b),
                capabilities: caps(b.id),
            }
        })
        .collect();
    // A plain shell, for a terminal beside an agent.
    let shell = std::env::var("SHELL").ok().filter(|s| Path::new(s).is_absolute()).unwrap_or_else(|| "/bin/sh".into());
    out.push(Harness {
        id: "shell".into(),
        name: "Terminal".into(),
        tier: Tier::Terminal,
        version: None,
        path: Some(shell),
        auth: AuthState::Unknown,
        capabilities: Capabilities::default(),
    });
    for g in generic_defs() {
        let exe = Path::new(&g.command[0]);
        let path = if exe.is_absolute() { Some(exe.to_path_buf()) } else { discover::find(&[g.command[0].as_str()], &[]) };
        out.push(Harness {
            id: g.id,
            name: g.name,
            tier: Tier::Generic,
            path: path.map(|p| p.display().to_string()),
            version: None,
            auth: AuthState::Unknown,
            capabilities: Capabilities::default(),
        });
    }
    out
}

fn exe_for(h: &Harness) -> Result<PathBuf, String> {
    h.path.as_ref().map(PathBuf::from).ok_or_else(|| format!("{} is not installed", h.name))
}

/// How to start a new session of `h`.
pub fn launch(h: &Harness, args: &[String]) -> Result<Plan, String> {
    let exe = exe_for(h)?.display().to_string();
    let plan = match h.id.as_str() {
        "claude" => {
            let id = crate::new_uuid();
            let mut argv = vec![exe, "--session-id".into(), id.clone()];
            argv.extend(args.iter().cloned());
            Plan { argv, harness_session: Some(id) }
        }
        _ if h.tier == Tier::Generic => {
            let def = generic_defs().into_iter().find(|g| g.id == h.id).ok_or("unknown harness")?;
            let mut argv = def.command;
            argv[0] = exe;
            argv.extend(args.iter().cloned());
            Plan { argv, harness_session: None }
        }
        _ => {
            let mut argv = vec![exe];
            argv.extend(args.iter().cloned());
            Plan { argv, harness_session: None }
        }
    };
    Ok(plan)
}

/// How to continue an ended session, when the harness supports it.
pub fn resume(h: &Harness, s: &Session) -> Result<Plan, String> {
    let exe = exe_for(h)?.display().to_string();
    match (h.id.as_str(), s.harness_session.as_deref()) {
        ("claude", Some(id)) => Ok(Plan { argv: vec![exe, "--resume".into(), id.into()], harness_session: Some(id.into()) }),
        _ => Err(format!("{} sessions cannot be resumed yet", h.name)),
    }
}

/// Claude Code's transcript for a session: `~/.claude/projects/<cwd with
/// "/" and "." as "-">/<session>.jsonl`.
pub fn claude_transcript(cwd: &str, session: &str) -> PathBuf {
    let slug: String = cwd.chars().map(|c| if c == '/' || c == '.' { '-' } else { c }).collect();
    crate::paths::home().join(".claude/projects").join(slug).join(format!("{session}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_launch_presets_the_session_id() {
        let h = Harness {
            id: "claude".into(),
            name: "Claude Code".into(),
            tier: Tier::Native,
            path: Some("/opt/claude".into()),
            version: None,
            auth: AuthState::Unknown,
            capabilities: caps("claude"),
        };
        let p = launch(&h, &["--model".into(), "opus".into()]).unwrap();
        assert_eq!(p.argv[..2], ["/opt/claude".to_string(), "--session-id".to_string()]);
        assert_eq!(p.argv[2], *p.harness_session.as_ref().unwrap());
        assert_eq!(p.argv[3..], ["--model".to_string(), "opus".to_string()]);
    }

    #[test]
    fn transcript_path_matches_claude_layout() {
        let p = claude_transcript("/home/ken/Projects/nyx.agents", "abc");
        assert!(p.ends_with(".claude/projects/-home-ken-Projects-nyx-agents/abc.jsonl"), "{}", p.display());
    }

    #[test]
    fn no_gemini_or_pi_forge() {
        assert!(BUILTINS.iter().all(|b| !b.id.contains("gemini") && !b.id.contains("forge")));
    }
}
