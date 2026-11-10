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
