//! Spawning from one long-lived thread.
//!
//! `PR_SET_PDEATHSIG` fires when the thread that forked the child exits, not
//! the process. A daemon that serves each client on its own thread would kill
//! sway as soon as the client that asked for it disconnected, so every child
//! is forked here instead.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::sync::{Mutex, OnceLock};

type Job = (Command, SyncSender<io::Result<Child>>);

static KEEPER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

fn keeper() -> &'static Mutex<Sender<Job>> {
    KEEPER.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("nyx-agent-workspace-spawn".into())
            .spawn(move || {
                for (mut cmd, reply) in rx {
                    let _ = reply.send(cmd.spawn());
                }
            })
            .expect("spawn thread");
        Mutex::new(tx)
    })
}

/// Spawn `cmd` in a new process group (or session, with `new_session`) that
/// gets `signal` when this process ends.
pub fn spawn(mut cmd: Command, new_session: bool, signal: libc::c_int) -> io::Result<Child> {
    unsafe {
        cmd.pre_exec(move || {
            if new_session {
                libc::setsid();
            } else {
                libc::setpgid(0, 0);
            }
            if libc::prctl(libc::PR_SET_PDEATHSIG, signal as libc::c_ulong, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            // The parent may have gone before prctl took effect.
            if libc::getppid() == 1 {
                return Err(io::Error::other("parent exited"));
            }
            Ok(())
        });
    }
    let (tx, rx) = sync_channel(1);
    keeper().lock().unwrap().send((cmd, tx)).map_err(|_| io::Error::other("spawn thread gone"))?;
    rx.recv().map_err(|_| io::Error::other("spawn thread gone"))?
}

fn pidfd(pid: u32) -> Option<libc::c_int> {
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::c_int, 0) } as libc::c_int;
    (fd >= 0).then_some(fd)
}

/// Kill process group `group` if this process ends first, however it ends.
///
/// The parent-death signal is no use for sway: its binary carries file
/// capabilities (`cap_sys_nice`), and exec clears the signal for those. A
/// forked guard waits on both processes instead.
pub fn guard(group: u32) {
    let (Some(owner), Some(target)) = (pidfd(std::process::id()), pidfd(group)) else { return };
    match unsafe { libc::fork() } {
        0 => unsafe {
            // Only raw system calls from here: this is a fork of a threaded process.
            libc::setsid();
            for fd in 0..1024 {
                if fd != owner && fd != target {
                    libc::close(fd);
                }
            }
            let mut fds = [
                libc::pollfd { fd: owner, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: target, events: libc::POLLIN, revents: 0 },
            ];
            loop {
                if libc::poll(fds.as_mut_ptr(), 2, -1) < 0 {
                    continue;
                }
                if fds[0].revents != 0 {
                    libc::kill(-(group as libc::c_int), libc::SIGKILL);
                    libc::_exit(0);
                }
                if fds[1].revents != 0 {
                    libc::_exit(0);
                }
            }
        },
        pid => {
            unsafe {
                libc::close(owner);
                libc::close(target);
            }
            if pid > 0 {
                // Reap it when it ends, so it never lingers as a zombie.
                std::thread::spawn(move || unsafe {
                    libc::waitpid(pid, std::ptr::null_mut(), 0);
                });
            }
        }
    }
}
