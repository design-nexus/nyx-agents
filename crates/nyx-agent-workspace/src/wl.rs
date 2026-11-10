//! Wayland connection helpers shared by input and capture.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Instant;

use wayland_client::{Connection, EventQueue};

/// Connect to one compositor's socket by path, ignoring `WAYLAND_DISPLAY`.
pub fn connect(socket: &Path) -> Result<Connection, String> {
    let stream = UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
    Connection::from_socket(stream).map_err(|e| e.to_string())
}

/// Dispatch events until `done` holds. False when `deadline` passed first.
pub fn dispatch_until<S>(
    queue: &mut EventQueue<S>,
    state: &mut S,
    deadline: Option<Instant>,
    done: impl Fn(&S) -> bool,
) -> Result<bool, String> {
    loop {
        queue.dispatch_pending(state).map_err(|e| e.to_string())?;
        if done(state) {
            return Ok(true);
        }
        queue.flush().map_err(|e| e.to_string())?;
        let Some(guard) = queue.prepare_read() else { continue };
        let timeout = match deadline {
            None => -1,
            Some(d) => {
                let left = d.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(false);
                }
                left.as_millis().clamp(1, i32::MAX as u128) as i32
            }
        };
        let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut pfd, 1, timeout) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.to_string());
        }
        if n > 0 {
            guard.read().map_err(|e| e.to_string())?;
        }
    }
}

/// Shared memory for a buffer: an anonymous file, never on disk.
pub fn memfd(len: usize) -> Result<std::fs::File, String> {
    use std::os::fd::FromRawFd;
    let fd = unsafe { libc::memfd_create(c"nyx-agent-workspace".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let f = unsafe { std::fs::File::from_raw_fd(fd) };
    f.set_len(len as u64).map_err(|e| e.to_string())?;
    Ok(f)
}
