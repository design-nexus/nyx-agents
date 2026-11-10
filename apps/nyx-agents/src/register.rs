//! `nyx-agents computer register [--remove] [HARNESS...]`: give harnesses that
//! only read MCP servers from their config the `nyx-computer` server, which
//! offers tools only in sessions that have an agent workspace.
//!
//! Each harness is changed through its own `mcp add` where it has one;
//! otherwise one key is added to its user config and the rest of the file is
//! kept. Claude Code, Codex and Muse need none of this: they get the server
//! per session.

use std::path::{Path, PathBuf};
use std::process::Command;

use nyx_agents_core::paths;
use serde_json::{Value, json};

const NAME: &str = "nyx-computer";
const HARNESSES: &[&str] = &["grok", "hermes", "antigravity", "cursor", "opencode", "crush"];

fn exe() -> String {
    let installed = paths::home().join(".local/bin/nyx-agents");
    if installed.is_file() {
        return installed.display().to_string();
    }
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "nyx-agents".into())
}

fn run_tool(argv: &[&str]) -> Result<(), String> {
    let out = Command::new(argv[0]).args(&argv[1..]).output().map_err(|e| format!("{}: {e}", argv[0]))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().last().unwrap_or("failed").to_string())
    }
}

/// Set (or with `None`, remove) `path` inside the JSON file, keeping everything else.
fn edit_json(file: &Path, path: &[&str], value: Option<Value>) -> Result<(), String> {
    let mut doc: Value = match std::fs::read_to_string(file) {
        Ok(t) => {
            serde_json::from_str(&t).map_err(|e| format!("{} is not plain JSON ({e}); add the server by hand", file.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.to_string()),
    };
    let (last, parents) = path.split_last().unwrap();
    let mut node = &mut doc;
    for p in parents {
        if !node[*p].is_object() {
            if value.is_none() {
                return Ok(());
            }
            node[*p] = json!({});
        }
        node = &mut node[*p];
    }
    let obj = node.as_object_mut().ok_or("unexpected config shape")?;
    match value {
        Some(v) => {
            obj.insert(last.to_string(), v);
        }
        None => {
            obj.remove(*last);
        }
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = file.with_extension("nyx-tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&doc).unwrap() + "\n").map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| e.to_string())
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| paths::home().join(".config"))
}

/// What registering changes for `harness`, in words.
fn target(harness: &str) -> &'static str {
    match harness {
        "grok" => "grok mcp add (~/.grok/config.toml)",
        "hermes" => "hermes mcp add",
        "antigravity" => "agy mcp add (~/.gemini/config/mcp_config.json)",
        "cursor" => "~/.cursor/mcp.json (Cursor CLI and oh-my-pi read it)",
        "opencode" => "~/.config/opencode/opencode.json",
        "crush" => "~/.config/crush/crush.json",
        _ => "",
    }
}

fn register(harness: &str, exe: &str, remove: bool) -> Result<(), String> {
    match (harness, remove) {
        ("grok", false) => run_tool(&["grok", "mcp", "add", NAME, exe, "--", "computer"]),
        ("grok", true) => run_tool(&["grok", "mcp", "remove", NAME]),
        ("hermes", false) => run_tool(&["hermes", "mcp", "add", NAME, "--command", exe, "--args", "computer"]),
        ("hermes", true) => run_tool(&["hermes", "mcp", "remove", NAME]),
        ("antigravity", false) => run_tool(&["agy", "mcp", "add", NAME, exe, "computer"]),
        ("antigravity", true) => run_tool(&["agy", "mcp", "remove", NAME]),
        ("cursor", _) => {
            let v = (!remove).then(|| json!({ "command": exe, "args": ["computer"] }));
            edit_json(&paths::home().join(".cursor/mcp.json"), &["mcpServers", NAME], v)?;
            if !remove {
                // Cursor CLI loads only MCP servers it has been told to trust.
                let _ = run_tool(&["cursor-agent", "mcp", "enable", NAME]);
            }
            Ok(())
        }
        ("opencode", _) => {
            let v = (!remove).then(|| json!({ "type": "local", "command": [exe, "computer"], "enabled": true }));
            edit_json(&config_home().join("opencode/opencode.json"), &["mcp", NAME], v)
        }
        ("crush", _) => {
            let v = (!remove).then(|| json!({ "type": "stdio", "command": exe, "args": ["computer"] }));
            edit_json(&config_home().join("crush/crush.json"), &["mcp", NAME], v)
        }
        _ => Err(format!("{harness} is not one that needs registering")),
    }
}

pub fn run(args: &[String]) -> i32 {
    let remove = args.iter().any(|a| a == "--remove");
    let installed: Vec<String> =
        nyx_agents_core::adapters::harnesses(false).into_iter().filter(|h| h.path.is_some()).map(|h| h.id).collect();
    let asked: Vec<&str> = args.iter().map(String::as_str).filter(|a| !a.starts_with("--")).collect();
    let list: Vec<&str> =
        if asked.is_empty() { HARNESSES.iter().copied().filter(|h| installed.iter().any(|i| i == h)).collect() } else { asked };
    if args.iter().any(|a| a == "--dry-run") {
        for h in &list {
            println!("{h:<12} {}", target(h));
        }
        return 0;
    }
    let exe = exe();
    let mut failed = 0;
    for h in list {
        match register(h, &exe, remove) {
            Ok(()) => println!("{h:<12} {}", if remove { "removed" } else { target(h) }),
            Err(e) => {
                failed += 1;
                eprintln!("{h:<12} failed: {e}");
            }
        }
    }
    i32::from(failed > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_edits_keep_the_rest_of_the_file() {
        let dir = std::env::temp_dir().join(format!("nyx-register-{}", std::process::id()));
        let file = dir.join("c.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, r#"{"theme":"dark","mcp":{"other":{"type":"local"}}}"#).unwrap();
        edit_json(&file, &["mcp", NAME], Some(json!({"type": "local"}))).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert!(v["mcp"]["other"].is_object() && v["mcp"][NAME].is_object());
        edit_json(&file, &["mcp", NAME], None).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(v["mcp"].get(NAME).is_none() && v["mcp"]["other"].is_object());
        std::fs::write(&file, "{ // jsonc\n}").unwrap();
        assert!(edit_json(&file, &["mcp", NAME], Some(json!({}))).is_err(), "comments are never rewritten");
        let new = dir.join("sub/new.json");
        edit_json(&new, &["mcpServers", NAME], Some(json!({"command": "x"}))).unwrap();
        assert!(new.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
