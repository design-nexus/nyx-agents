//! Recognising the session a harness started when it can't be given an id
//! up front: the first new session record in the same folder after the
//! launch. Read only.
//!
//! - Codex CLI: rollouts, see [`crate::codex`].
//! - Cursor CLI: `~/.config/cursor/chats/<hash>/<chatId>/meta.json` with
//!   `cwd`, `createdAtMs`, `hasConversation`; resumed with `--resume <chatId>`.
//! - oh-my-pi: `~/.omp/agent/sessions/<folder>/<time>_<id>.jsonl`, where the
//!   folder is the path from home with `/` as `-` (`~/Projects` is
//!   `-Projects`, `/tmp` is `-tmp`); resumed with `-r <id>`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::paths::home;

fn cursor_chats() -> PathBuf {
    home().join(".config/cursor/chats")
}

/// oh-my-pi's folder for a working directory.
pub fn omp_dir(cwd: &str) -> PathBuf {
    let h = home();
    let rel =
        Path::new(cwd).strip_prefix(&h).map(|r| r.to_path_buf()).unwrap_or_else(|_| PathBuf::from(cwd.trim_start_matches('/')));
    let slug = format!("-{}", rel.to_string_lossy().replace('/', "-"));
    h.join(".omp/agent/sessions").join(slug)
}

fn subdirs(d: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(d).map(|it| it.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

fn mtime_ms(p: &Path) -> u64 {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Folders to watch while waiting for `harness` to write its session.
pub fn watch_dirs(harness: &str, cwd: &str, since: u64) -> Vec<PathBuf> {
    match harness {
        "codex" => crate::codex::day_dirs(since)
            .into_iter()
            .flat_map(|d| [d.parent().map(Path::to_path_buf), Some(d)])
            .flatten()
            .collect(),
        "cursor" => {
            let mut v = vec![cursor_chats()];
            v.extend(subdirs(&cursor_chats()));
            v
        }
        "omp" => {
            let d = omp_dir(cwd);
            vec![d.parent().map(Path::to_path_buf).unwrap_or_default(), d]
        }
        // SQLite writes land in the database's folder (the file and its -wal).
        h => crate::sqlite_sessions::db_path(h).and_then(|p| p.parent().map(Path::to_path_buf)).into_iter().collect(),
    }
}

/// The session id `harness` started in `cwd` after `since`, skipping `taken`.
/// `before` holds the ids a SQLite harness had in `cwd` before the launch.
pub fn find(
    harness: &str,
    cwd: &str,
    since: u64,
    taken: &[String],
    before: &std::collections::HashSet<String>,
) -> Option<String> {
    let fresh = |t: u64| t + 2_000 >= since;
    match harness {
        "codex" => crate::codex::find_session(cwd, since, taken),
        "cursor" => {
            let mut found: Vec<(u64, String)> = Vec::new();
            for hash in subdirs(&cursor_chats()) {
                for chat in subdirs(&hash) {
                    let Ok(b) = std::fs::read(chat.join("meta.json")) else { continue };
                    let Ok(m) = serde_json::from_slice::<Value>(&b) else { continue };
                    let created = m["createdAtMs"].as_u64().unwrap_or(0);
                    let id = chat.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    if m["cwd"].as_str() == Some(cwd) && fresh(created) && !taken.contains(&id) {
                        found.push((created, id));
                    }
                }
            }
            found.sort();
            found.into_iter().next().map(|(_, id)| id)
        }
        "omp" => {
            let mut found: Vec<(u64, String)> = std::fs::read_dir(omp_dir(cwd))
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "jsonl") && fresh(mtime_ms(p)))
                .filter_map(|p| {
                    let stem = p.file_stem()?.to_string_lossy().into_owned();
                    let id = stem.split_once('_')?.1.to_string();
                    (!taken.contains(&id)).then(|| (mtime_ms(&p), id))
                })
                .collect();
            found.sort();
            found.into_iter().next().map(|(_, id)| id)
        }
        h if crate::sqlite_sessions::HARNESSES.contains(&h) => crate::sqlite_sessions::find_new(h, cwd, before, taken),
        _ => None,
    }
}

/// Whether `harness` has a conversation to resume for session `id`.
pub fn resumable(harness: &str, cwd: &str, id: &str, since: u64) -> bool {
    match harness {
        "codex" => crate::codex::has_session(id, since),
        "cursor" => subdirs(&cursor_chats()).iter().any(|h| {
            std::fs::read(h.join(id).join("meta.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .is_some_and(|m| m["hasConversation"].as_bool() == Some(true))
        }),
        "omp" => std::fs::read_dir(omp_dir(cwd))
            .map(|it| it.flatten().any(|e| e.file_name().to_string_lossy().ends_with(&format!("_{id}.jsonl"))))
            .unwrap_or(false),
        h if crate::sqlite_sessions::HARNESSES.contains(&h) => crate::sqlite_sessions::exists(h, cwd, id),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omp_folder_names() {
        let h = home();
        assert!(omp_dir(&h.join("Projects/web").to_string_lossy()).ends_with(".omp/agent/sessions/-Projects-web"));
        assert!(omp_dir(&h.to_string_lossy()).ends_with(".omp/agent/sessions/-"));
        assert!(omp_dir("/tmp").ends_with(".omp/agent/sessions/-tmp"));
    }
}
