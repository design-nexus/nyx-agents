//! Only the user's own processes may talk to the manager or a holder.

use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

pub fn same_user(s: &UnixStream) -> bool {
    let mut cred = libc::ucred { pid: 0, uid: u32::MAX, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe {
        libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
    };
    r == 0 && cred.uid == unsafe { libc::getuid() }
}
