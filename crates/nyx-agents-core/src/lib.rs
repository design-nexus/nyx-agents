//! Nyx Agent Manager internals shared by the daemon, the app and the CLI.

pub mod adapters;
pub mod claude;
pub mod codex;
pub mod discover;
pub mod git;
pub mod hub;
pub mod inotify;
pub mod paths;
pub mod sanitize;
pub mod sessions;
pub mod sqlite_sessions;
pub mod store;
pub mod worktree;

/// A random identifier: 128 bits from the kernel, as hex.
pub fn new_id() -> String {
    let mut b = [0u8; 16];
    let mut filled = 0;
    while filled < b.len() {
        let n = unsafe { libc::getrandom(b[filled..].as_mut_ptr().cast(), b.len() - filled, 0) };
        if n > 0 {
            filled += n as usize;
        }
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A random RFC 4122 version 4 UUID (for harnesses that take one).
pub fn new_uuid() -> String {
    let mut h = new_id().into_bytes();
    h[12] = b'4';
    h[16] = b"89ab"[(h[16] as usize) % 4];
    let s = String::from_utf8(h).unwrap();
    format!("{}-{}-{}-{}-{}", &s[0..8], &s[8..12], &s[12..16], &s[16..20], &s[20..32])
}

/// Tests that change environment variables hold this, so they don't race.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    #[test]
    fn uuids_look_right() {
        let u = super::new_uuid();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!("89ab".contains(&u[19..20]));
        assert_ne!(u, super::new_uuid());
    }
}
