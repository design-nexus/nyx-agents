//! Harnesses that keep their sessions in SQLite, opened read-only.
//!
//! Timestamps differ between them (and some tables were empty here to check
//! units against), so the session a launch created is recognised as the new
//! top-level session id in the same folder that wasn't there just before.
//!
//! | Harness | Database | Folder column | Resumed with |
//! |---|---|---|---|
//! | OpenCode | `~/.local/share/opencode/opencode.db` `session` | `directory` | `-s <id>` |
//! | Hermes | `~/.hermes/state.db` `sessions` | `cwd` | `-r <id>` |
//! | Muse | `~/.local/share/muse/session-index.db` `sessions` | `workspace_root` | `resume <id>` |
//! | Antigravity | `~/.gemini/antigravity-cli/conversation_summaries.db` | `workspace_uris` (file URIs) | `--conversation <id>` |

use std::collections::HashSet;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};

use crate::paths::home;

pub const HARNESSES: &[&str] = &["opencode", "hermes", "muse", "antigravity"];

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"))
}

pub fn db_path(harness: &str) -> Option<PathBuf> {
    Some(match harness {
        "opencode" => data_home().join("opencode/opencode.db"),
        "hermes" => home().join(".hermes/state.db"),
        "muse" => data_home().join("muse/session-index.db"),
        "antigravity" => home().join(".gemini/antigravity-cli/conversation_summaries.db"),
        _ => return None,
    })
}

fn open(harness: &str) -> Option<Connection> {
    let path = db_path(harness)?;
    let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()?;
    let _ = c.busy_timeout(std::time::Duration::from_millis(250));
    Some(c)
}

/// `file://` URI of a folder, as Antigravity records workspaces.
fn file_uri(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Top-level session ids recorded for `cwd`, oldest first.
pub fn ids(harness: &str, cwd: &str) -> Vec<String> {
    let Some(c) = open(harness) else { return Vec::new() };
    let (sql, arg) = match harness {
        "opencode" => {
            ("select id from session where directory = ?1 and parent_id is null order by time_created", cwd.to_string())
        }
        "hermes" => ("select id from sessions where cwd = ?1 and parent_session_id is null order by started_at", cwd.to_string()),
        "muse" => (
            "select session_id from sessions where workspace_root = ?1 and coalesce(msp_parent_session_id, '') = '' order by created_at_us",
            cwd.to_string(),
        ),
        "antigravity" => (
            "select conversation_id from conversation_summaries where workspace_uris like ?1 escape '\\' and coalesce(parent_conversation_id, '') = '' order by last_modified_time",
            format!("%\"{}\"%", file_uri(cwd).replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")),
        ),
        _ => return Vec::new(),
    };
    let Ok(mut stmt) = c.prepare(sql) else { return Vec::new() };
    stmt.query_map([arg], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// The session ids already there, taken just before a launch.
pub fn snapshot(harness: &str, cwd: &str) -> HashSet<String> {
    ids(harness, cwd).into_iter().collect()
}

/// The new session in `cwd` since `before`, skipping ids already claimed.
pub fn find_new(harness: &str, cwd: &str, before: &HashSet<String>, taken: &[String]) -> Option<String> {
    ids(harness, cwd).into_iter().find(|id| !before.contains(id) && !taken.contains(id))
}

pub fn exists(harness: &str, cwd: &str, id: &str) -> bool {
    ids(harness, cwd).iter().any(|i| i == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_new_opencode_session_in_the_folder() {
        let t = tempfile::tempdir().unwrap();
        // SAFETY: no other test reads XDG_DATA_HOME for OpenCode.
        unsafe { std::env::set_var("XDG_DATA_HOME", t.path()) };
        let p = db_path("opencode").unwrap();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "create table session (id text, directory text, parent_id text, time_created integer);
             insert into session values ('old', '/w', null, 1);",
        )
        .unwrap();
        let before = snapshot("opencode", "/w");
        c.execute_batch(
            "insert into session values ('child', '/w', 'new', 3);
             insert into session values ('elsewhere', '/x', null, 4);
             insert into session values ('new', '/w', null, 2);",
        )
        .unwrap();
        assert_eq!(find_new("opencode", "/w", &before, &[]).as_deref(), Some("new"));
        assert_eq!(find_new("opencode", "/w", &before, &["new".into()]), None);
        assert!(exists("opencode", "/w", "new"));
    }

    #[test]
    fn antigravity_workspace_uris() {
        assert_eq!(file_uri("/home/ken/My Project"), "file:///home/ken/My%20Project");
    }
}
