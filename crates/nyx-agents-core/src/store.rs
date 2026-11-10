//! Session records on disk: `~/.local/state/nyx-agents/sessions/<id>/`.
//!
//! - `session.json`: the [`Session`] record.
//! - `screen.snap`: the terminal (screen and scrollback), written by the holder.
//! - `holder.log`: the holder's own log, capped.

use std::path::PathBuf;

use nyx_agents_proto::Session;

use crate::paths;

pub fn load_all() -> Vec<Session> {
    let Ok(dir) = std::fs::read_dir(paths::sessions_dir()) else { return Vec::new() };
    let mut out: Vec<Session> = dir
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("session.json")).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    out.sort_by_key(|s| s.created);
    out
}

pub fn save(s: &Session) -> std::io::Result<()> {
    let dir = paths::session_dir(&s.id);
    paths::private_dir(&dir)?;
    paths::write_private(&dir.join("session.json"), &serde_json::to_vec_pretty(s).unwrap())
}

pub fn remove(id: &str) -> std::io::Result<()> {
    // Ids are generated hex; refuse anything that could escape the directory.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "bad session id"));
    }
    std::fs::remove_dir_all(paths::session_dir(id))
}

pub fn snapshot_path(id: &str) -> PathBuf {
    paths::session_dir(id).join("screen.snap")
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit())
}
