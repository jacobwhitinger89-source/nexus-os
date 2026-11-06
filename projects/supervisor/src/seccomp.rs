//! Seccomp deny-list applied to every project process.
//!
//! Project services already run as unprivileged users without capabilities;
//! this filter is defence in depth against kernel attack surface that an
//! application server never needs. Denied calls fail with EPERM (ENOSYS for
//! io_uring so runtimes fall back to classic I/O). Non-x86-64 syscall ABIs
//! (x32, i386) are refused outright.

use libc::sock_filter;

const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
const X32_SYSCALL_BIT: u32 = 0x4000_0000;

const LD_W_ABS: u16 = 0x20;
const JEQ_K: u16 = 0x15;
const JGE_K: u16 = 0x35;
const RET_K: u16 = 0x06;

const RET_ALLOW: u32 = 0x7fff_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const RET_KILL_PROCESS: u32 = 0x8000_0000;

/// (syscall, errno)
fn denied() -> Vec<(u32, u32)> {
    let eperm = libc::EPERM as u32;
    let enosys = libc::ENOSYS as u32;
    let mut v: Vec<(u32, u32)> = [
        libc::SYS_ptrace,
        libc::SYS_process_vm_writev,
        libc::SYS_kexec_load,
        libc::SYS_kexec_file_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_userfaultfd,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_unshare,
        libc::SYS_setns,
        libc::SYS_fsopen,
        libc::SYS_fsmount,
        libc::SYS_fsconfig,
        libc::SYS_move_mount,
        libc::SYS_open_tree,
        libc::SYS_swapon,
        libc::SYS_swapoff,
        libc::SYS_reboot,
        libc::SYS_settimeofday,
        libc::SYS_clock_settime,
        libc::SYS_clock_adjtime,
        libc::SYS_adjtimex,
        libc::SYS_acct,
        libc::SYS_quotactl,
        libc::SYS_open_by_handle_at,
        libc::SYS_name_to_handle_at,
        libc::SYS_iopl,
        libc::SYS_ioperm,
        libc::SYS_syslog,
        libc::SYS_vhangup,
    ]
    .iter()
    .map(|n| (*n as u32, eperm))
    .collect();
    for n in [
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ] {
        v.push((n as u32, enosys));
    }
    v
}

fn stmt(code: u16, k: u32) -> sock_filter {
    sock_filter {
        code,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u16, k: u32, jt: u8, jf: u8) -> sock_filter {
    sock_filter { code, jt, jf, k }
}

/// Build the BPF program. Built before fork; installed in the child.
pub fn program() -> Vec<sock_filter> {
    let deny = denied();
    let mut p = vec![
        stmt(LD_W_ABS, 4), // seccomp_data.arch
        jump(JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
        stmt(RET_K, RET_KILL_PROCESS),
        stmt(LD_W_ABS, 0), // seccomp_data.nr
        jump(JGE_K, X32_SYSCALL_BIT, 0, 1),
        stmt(RET_K, RET_ERRNO | libc::EPERM as u32),
    ];
    // Layout: n compares, ALLOW, then n RET slots. Compare i jumps to RET
    // slot i: past the remaining n-1-i compares, the ALLOW and i slots,
    // i.e. a forward distance of n. (BPF jump offsets are 8-bit, n < 255.)
    let n = deny.len();
    for (nr, _) in &deny {
        p.push(jump(JEQ_K, *nr, n as u8, 0));
    }
    p.push(stmt(RET_K, RET_ALLOW));
    for (_, errno) in &deny {
        p.push(stmt(RET_K, RET_ERRNO | *errno));
    }
    p
}

/// Install `prog`. Must be called after PR_SET_NO_NEW_PRIVS.
/// Async-signal-safe: used inside `pre_exec`.
pub fn install(prog: &[sock_filter]) -> std::io::Result<()> {
    let fprog = libc::sock_fprog {
        len: prog.len() as u16,
        filter: prog.as_ptr() as *mut sock_filter,
    };
    let r = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER as libc::c_ulong,
            &fprog as *const libc::sock_fprog as libc::c_ulong,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    #[test]
    fn jump_targets_in_range() {
        let p = program();
        let n = denied().len();
        assert_eq!(p.len(), 6 + n + 1 + n);
        for i in 0..n {
            let at = 6 + i;
            let target = at + 1 + p[at].jt as usize;
            assert_eq!(p[target].code, RET_K);
            assert_eq!(p[target].k & 0xffff_0000, RET_ERRNO);
        }
    }

    /// Run a child under the filter: unshare(2) must fail with EPERM while
    /// ordinary work (exec of a shell, writing output) still succeeds.
    #[test]
    fn filter_blocks_unshare_in_child() {
        let prog = program();
        let mut cmd = Command::new("/bin/sh");
        cmd.args([
            "-c",
            "echo alive; unshare -r true 2>/dev/null && echo escaped || echo blocked",
        ]);
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                install(&prog)
            });
        }
        let out = cmd.output().expect("spawn");
        let s = String::from_utf8_lossy(&out.stdout);
        assert!(s.contains("alive"), "{s}");
        assert!(!s.contains("escaped"), "{s}");
    }
}
