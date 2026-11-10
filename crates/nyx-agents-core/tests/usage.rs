//! The usage scanners on made-up records in a temporary folder.

use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use nyx_agents_core::usage;
use serde_json::json;

/// The scanners read `NYX_AGENTS_CACHE_DIR` and `XDG_CONFIG_HOME`; tests share them.
static ENV: Mutex<()> = Mutex::new(());

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64
}

fn write_lines(path: &Path, lines: &[serde_json::Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

fn scratch() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("NYX_AGENTS_CACHE_DIR", dir.path().join("cache"));
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    }
    (dir, guard)
}

#[test]
fn claude_counts_each_message_once() {
    let (dir, _g) = scratch();
    let base = dir.path().join("claude");
    let t = now_ms();
    let session = vec![
        json!({ "type": "user", "uuid": "u1", "timestamp": t, "cwd": "/p/app", "message": { "role": "user", "content": "fix the parser" } }),
        json!({ "type": "assistant", "timestamp": t, "requestId": "r1", "message": { "id": "m1", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{ "type": "tool_use", "name": "Bash" }],
            "usage": { "input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 2 } } }),
        // The same message's next content block repeats its usage.
        json!({ "type": "assistant", "timestamp": t, "requestId": "r1", "message": { "id": "m1", "role": "assistant", "model": "claude-opus-5-5",
            "content": [{ "type": "text", "text": "done" }],
            "usage": { "input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 100, "cache_creation_input_tokens": 2 } } }),
        json!({ "type": "user", "timestamp": t, "message": { "role": "user", "content": [{ "type": "tool_result", "content": "ok" }] } }),
        json!({ "type": "user", "timestamp": t, "message": { "role": "user", "content": "<command-name>/clear</command-name><command-args></command-args>" } }),
    ];
    let main = base.join("projects/-p-app/aaaa.jsonl");
    write_lines(&main, &session);
    // A resumed session copies the earlier rows into a new transcript.
    write_lines(&base.join("projects/-p-app/bbbb.jsonl"), &session);
    write_lines(
        &base.join("projects/-p-app/aaaa/subagents/agent-1.jsonl"),
        &[json!({ "type": "assistant", "timestamp": t, "requestId": "r2", "message": { "id": "m2", "role": "assistant", "model": "claude-sonnet-5",
            "content": [], "usage": { "input_tokens": 7, "output_tokens": 3 } } })],
    );

    let (u, _) = usage::claude::scan(&base, false);
    assert!(u.has_records);
    assert_eq!(u.today.prompts, 1);
    assert_eq!(u.today.steps, 2);
    assert_eq!(u.today.tokens, 17 + 10);
    assert_eq!(u.today.cached, 100);
    assert_eq!(u.sessions.len(), 2, "subagent transcripts aren't sessions");
    assert_eq!(u.sessions[0].title, "fix the parser");
    assert_eq!(u.sessions[0].cwd, "/p/app");
    assert_eq!(u.models[0].name, "Claude Opus 5.5");
    assert_eq!(u.models[0].today_prompts, 1);
    assert!(u.models.iter().any(|m| m.name == "Claude Sonnet 5" && m.steps == 1));
    assert!(u.tools.iter().any(|t| t.name == "Bash"));
    assert_eq!(u.days.len(), 7);
    assert_eq!(u.days[6].prompts, 1);
    assert!(!u.quota.enabled, "quota checks are off until turned on");

    // The next scan reads only what was appended.
    write_lines(&main, &[
        json!({ "type": "user", "uuid": "u2", "timestamp": t, "message": { "role": "user", "content": "and the tests" } }),
        json!({ "type": "assistant", "timestamp": t, "requestId": "r3", "message": { "id": "m3", "role": "assistant", "model": "claude-opus-5-5",
            "content": [], "usage": { "input_tokens": 1, "output_tokens": 1 } } }),
    ]);
    let (u, _) = usage::claude::scan(&base, false);
    assert_eq!(u.today.prompts, 2);
    assert_eq!(u.today.steps, 3);
}

#[test]
fn codex_counts_turns_and_calls() {
    let (dir, _g) = scratch();
    let base = dir.path().join("codex");
    let id = "7f3a91c2-58b1-4e0a-9c55-3f1d2a6b9e04";
    let t = now_ms();
    write_lines(&base.join(format!("sessions/2026/11/10/rollout-{id}.jsonl")), &[
        json!({ "type": "session_meta", "timestamp": t, "payload": { "id": id, "cwd": "/p/files" } }),
        json!({ "type": "turn_context", "timestamp": t, "payload": { "model": "gpt-5.5" } }),
        json!({ "type": "event_msg", "timestamp": t, "payload": { "type": "task_started" } }),
        json!({ "type": "response_item", "timestamp": t, "payload": { "type": "function_call", "name": "shell" } }),
        json!({ "type": "event_msg", "timestamp": t, "payload": { "type": "token_count",
            "info": { "last_token_usage": { "input_tokens": 100, "output_tokens": 20, "cached_input_tokens": 80 } } } }),
        json!({ "type": "event_msg", "timestamp": t, "payload": { "type": "token_count", "info": null } }),
    ]);
    write_lines(&base.join("session_index.jsonl"), &[json!({ "id": id, "thread_name": "Fix the CSV parser" })]);

    let (u, history) = usage::codex::scan(&base, false);
    assert_eq!(history.len(), 1);
    assert!(u.has_records);
    assert_eq!((u.today.prompts, u.today.steps, u.today.tokens, u.today.cached), (1, 1, 120, 80));
    assert_eq!(u.sessions.len(), 1);
    assert_eq!(u.sessions[0].title, "Fix the CSV parser");
    assert_eq!(u.sessions[0].cwd, "/p/files");
    assert_eq!(u.sessions[0].model.as_deref(), Some("gpt-5.5"));
    assert_eq!(u.models[0].name, "gpt-5.5");
    assert_eq!(u.tools[0].name, "shell");
}

#[test]
fn quotas_only_when_turned_on() {
    let (dir, _g) = scratch();
    assert!(!usage::quota::enabled("claude"));
    std::fs::create_dir_all(dir.path().join("config/nyx-agents")).unwrap();
    std::fs::write(dir.path().join("config/nyx-agents/config.toml"), "[usage]\nquotas = [\"codex\"]\n").unwrap();
    assert!(usage::quota::enabled("codex"));
    assert!(!usage::quota::enabled("claude"));
    // A failed fetch keeps the error and isn't retried for five minutes.
    let q = usage::quota::get("codex", false, 180.0, || Err("offline".into()));
    assert!(q.enabled && q.windows.is_empty());
    assert_eq!(q.error.as_deref(), Some("offline"));
    let q = usage::quota::get("codex", false, 180.0, || panic!("asked again too soon"));
    assert_eq!(q.error.as_deref(), Some("offline"));
}
