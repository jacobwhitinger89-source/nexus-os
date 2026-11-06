//! Process sandbox applied between fork and exec for every project process
//! (services, builds, source extraction, health-check commands).
//!
//! Order inside the child (still root until step 5):
//!   1. new session (own process group, no controlling terminal)
//!   2. join the cgroup leaf
//!   3. private mount namespace with its own /tmp
//!   4. resource limits (open files, no core dumps)
//!   5. drop to the project user and group, no supplementary groups
//!   6. PR_SET_NO_NEW_PRIVS, then the seccomp filter
//!
//! When nexusd itself is not root (development host) steps 2, 3 and 5 are
//! skipped and `Isolation::Limited` is reported.

use std::ffi::CString;
use std::io;
use std::os::unix::process::CommandExt;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    Full,
    Limited,
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub struct Sandbox {
    pub uid: u32,
    pub gid: u32,
    pub cgroup: Option<std::path::PathBuf>,
    pub private_tmp: bool,
    pub nofile: u64,
}

fn check(r: libc::c_int) -> io::Result<()> {
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn apply(cmd: &mut Command, sb: &Sandbox) -> Isolation {
    let root = is_root();
    let procs = sb.cgroup.as_ref().filter(|_| root).and_then(|d| {
        CString::new(d.join("cgroup.procs").into_os_string().into_encoded_bytes()).ok()
    });
    let in_cgroup = procs.is_some();
    let switch_user = root && sb.uid != 0;
    let private_tmp = root && sb.private_tmp;
    let (uid, gid, nofile) = (sb.uid, sb.gid, sb.nofile);
    let filter = crate::seccomp::program();

    unsafe {
        cmd.pre_exec(move || {
            check(libc::setsid())?;
            if let Some(p) = &procs {
                let fd = libc::open(p.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC);
                check(fd)?;
                let r = libc::write(fd, b"0".as_ptr() as *const libc::c_void, 1);
                libc::close(fd);
                if r != 1 {
                    return Err(io::Error::last_os_error());
                }
            }
            if private_tmp {
                check(libc::unshare(libc::CLONE_NEWNS))?;
                check(libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                ))?;
                check(libc::mount(
                    c"tmpfs".as_ptr(),
                    c"/tmp".as_ptr(),
                    c"tmpfs".as_ptr(),
                    libc::MS_NOSUID | libc::MS_NODEV,
                    c"size=64m,mode=1777".as_ptr() as *const libc::c_void,
                ))?;
            }
            let nf = libc::rlimit {
                rlim_cur: nofile,
                rlim_max: nofile,
            };
            check(libc::setrlimit(libc::RLIMIT_NOFILE, &nf))?;
            let core = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            check(libc::setrlimit(libc::RLIMIT_CORE, &core))?;
            if switch_user {
                let groups = [gid as libc::gid_t];
                check(libc::setgroups(1, groups.as_ptr()))?;
                check(libc::setresgid(gid, gid, gid))?;
                check(libc::setresuid(uid, uid, uid))?;
                if libc::geteuid() != uid || libc::getegid() != gid {
                    return Err(io::Error::from_raw_os_error(libc::EPERM));
                }
            }
            check(libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0))?;
            crate::seccomp::install(&filter)?;
            Ok(())
        });
    }
    if switch_user && in_cgroup {
        Isolation::Full
    } else {
        Isolation::Limited
    }
}
