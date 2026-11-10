//! Where things live. Runtime sockets are private to the user (0700).

use std::path::PathBuf;

fn env_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `$XDG_RUNTIME_DIR/nyx-agents` (`NYX_AGENTS_RUNTIME_DIR` overrides).
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NYX_AGENTS_RUNTIME_DIR") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("nyx-agents-{}", unsafe { libc::getuid() })));
    base.join("nyx-agents")
}

pub fn control_socket() -> PathBuf {
    runtime_dir().join("agents.sock")
}

pub fn session_socket(id: &str) -> PathBuf {
    runtime_dir().join("s").join(format!("{id}.sock"))
}

pub fn hub_file() -> PathBuf {
    runtime_dir().join("hub.json")
}

/// `~/.local/state/nyx-agents` (`NYX_AGENTS_STATE_DIR` overrides).
pub fn state_dir() -> PathBuf {
    std::env::var_os("NYX_AGENTS_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env_dir("XDG_STATE_HOME", ".local/state").join("nyx-agents"))
}

pub fn sessions_dir() -> PathBuf {
    state_dir().join("sessions")
}

pub fn session_dir(id: &str) -> PathBuf {
    sessions_dir().join(id)
}

/// `~/.cache/nyx-agents` (`NYX_AGENTS_CACHE_DIR` overrides).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("NYX_AGENTS_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| env_dir("XDG_CACHE_HOME", ".cache").join("nyx-agents"))
}

/// `~/.config/nyx-agents`.
pub fn config_dir() -> PathBuf {
    env_dir("XDG_CONFIG_HOME", ".config").join("nyx-agents")
}

/// Create a directory readable only by the user.
pub fn private_dir(p: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(p)?;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
}

/// Write a file atomically (temp file and rename), readable only by the user.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(data)?;
        f.sync_data()?;
    }
    std::fs::rename(&tmp, path)
}
