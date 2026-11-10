//! A minimal inotify watcher, so following transcripts never polls files.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::time::Duration;

pub struct Watcher {
    fd: OwnedFd,
}

impl Watcher {
    pub fn new() -> std::io::Result<Self> {
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { fd: unsafe { OwnedFd::from_raw_fd(fd) } })
    }

    /// Watch a file or directory for writes, creations and renames. Adding
    /// the same path twice is harmless. False when the path does not exist.
    pub fn add(&self, path: &Path) -> bool {
        let Ok(c) = CString::new(path.as_os_str().as_encoded_bytes()) else { return false };
        let mask = libc::IN_MODIFY | libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_CLOSE_WRITE;
        unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), mask) >= 0 }
    }

    /// Wait up to `timeout` for anything to change. True if something did.
    pub fn wait(&self, timeout: Duration) -> bool {
        let mut pfd = libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let r = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis().min(i32::MAX as u128) as i32) };
        if r <= 0 {
            return false;
        }
        // Drain; callers re-read state rather than interpreting events.
        let mut buf = [0u8; 4096];
        while unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
        true
    }
}
