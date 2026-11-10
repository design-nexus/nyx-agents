//! Spawn a program on a new pseudo-terminal.
//!
//! The child gets its own session with the pty as its controlling terminal.
//! The caller owns the master side: read the program's output from it, write
//! input to it, and resize it. Programs are started from an argv, never
//! through a shell string.

use std::ffi::{CStr, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// A terminal size in cells and pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
    pub width_px: u16,
    pub height_px: u16,
}

/// The master side of a pty with a program running on it.
pub struct Pty {
    master: File,
    child: Child,
}

/// What to run.
pub struct Spawn<'a> {
    pub argv: &'a [&'a OsStr],
    pub cwd: Option<&'a Path>,
    /// Extra environment on top of the current one.
    pub env: &'a [(&'a OsStr, &'a OsStr)],
    /// Variables to remove from the inherited environment.
    pub env_remove: &'a [&'a OsStr],
    pub size: Size,
}

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

fn set_size(fd: RawFd, s: Size) -> io::Result<()> {
    let ws = libc::winsize { ws_row: s.rows.max(1), ws_col: s.cols.max(1), ws_xpixel: s.width_px, ws_ypixel: s.height_px };
    cvt(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) }).map(drop)
}

fn open_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let master = cvt(unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) })?;
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    cvt(unsafe { libc::grantpt(master.as_raw_fd()) })?;
    cvt(unsafe { libc::unlockpt(master.as_raw_fd()) })?;
    let mut name = [0 as libc::c_char; 128];
    let r = unsafe { libc::ptsname_r(master.as_raw_fd(), name.as_mut_ptr(), name.len()) };
    if r != 0 {
        return Err(io::Error::from_raw_os_error(r));
    }
    let name = unsafe { CStr::from_ptr(name.as_ptr()) };
    let slave = cvt(unsafe { libc::open(name.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) })?;
    Ok((master, unsafe { OwnedFd::from_raw_fd(slave) }))
}

impl Pty {
    pub fn spawn(s: &Spawn) -> io::Result<Self> {
        let Some((prog, args)) = s.argv.split_first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty argv"));
        };
        let (master, slave) = open_pair()?;
        set_size(master.as_raw_fd(), s.size)?;

        let mut cmd = Command::new(prog);
        cmd.args(args);
        if let Some(cwd) = s.cwd {
            cmd.current_dir(cwd);
        }
        for k in s.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in s.env {
            cmd.env(k, v);
        }
        let stdio = |fd: &OwnedFd| fd.try_clone().map(Stdio::from);
        cmd.stdin(stdio(&slave)?).stdout(stdio(&slave)?).stderr(stdio(&slave)?);
        unsafe {
            cmd.pre_exec(|| {
                // New session, with the pty (already on stdin) as its terminal.
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                // Undo signal state a GUI parent may have changed.
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigprocmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
                for sig in [libc::SIGPIPE, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGCHLD, libc::SIGHUP] {
                    libc::signal(sig, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let child = cmd.spawn()?;
        drop(slave);
        Ok(Self { master: File::from(master), child })
    }

    /// The master side, for reading output and writing input.
    pub fn master(&self) -> &File {
        &self.master
    }

    /// A second handle on the master, for a reader thread.
    pub fn try_clone_master(&self) -> io::Result<File> {
        self.master.try_clone()
    }

    pub fn resize(&self, s: Size) -> io::Result<()> {
        set_size(self.master.as_raw_fd(), s)
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// The foreground process group on the terminal (the running program).
    pub fn foreground_pgid(&self) -> Option<i32> {
        let p = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        (p > 0).then_some(p)
    }

    /// Exit status if the program has finished.
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait()
    }

    /// Send a signal to the program's whole process group.
    pub fn signal(&self, sig: libc::c_int) -> io::Result<()> {
        cvt(unsafe { libc::kill(-(self.child.id() as libc::pid_t), sig) }).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn runs_a_program_on_a_tty() {
        let argv = [OsStr::new("sh"), OsStr::new("-c"), OsStr::new("stty size; tty -s && echo tty")];
        let mut pty = Pty::spawn(&Spawn {
            argv: &argv,
            cwd: None,
            env: &[],
            env_remove: &[],
            size: Size { cols: 91, rows: 17, ..Default::default() },
        })
        .unwrap();
        let mut out = String::new();
        let mut buf = [0u8; 256];
        let mut master = pty.try_clone_master().unwrap();
        // EIO marks the slave closing once the program exits.
        while let Ok(n) = master.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        assert!(pty.wait().unwrap().success());
        assert!(out.contains("17 91"), "{out:?}");
        assert!(out.contains("tty"), "{out:?}");
    }

    #[test]
    fn input_reaches_the_program() {
        let argv = [OsStr::new("sh"), OsStr::new("-c"), OsStr::new("read x; echo got:$x")];
        let mut pty = Pty::spawn(&Spawn {
            argv: &argv,
            cwd: None,
            env: &[],
            env_remove: &[],
            size: Size { cols: 80, rows: 24, ..Default::default() },
        })
        .unwrap();
        pty.master().write_all(b"hello\r").unwrap();
        let mut out = String::new();
        let mut buf = [0u8; 256];
        let mut master = pty.try_clone_master().unwrap();
        while let Ok(n) = master.read(&mut buf) {
            if n == 0 {
                break;
            }
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        pty.wait().unwrap();
        assert!(out.contains("got:hello"), "{out:?}");
    }
}
