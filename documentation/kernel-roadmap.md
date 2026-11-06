# Research track: a NEXUS kernel in Rust

NEXUS OS ships the **Linux** kernel. This document is a research roadmap
only; nothing here is implemented, and the server product never depends on
it.

## Interfaces NEXUS userspace relies on today
A replacement kernel would need to provide (or NEXUS would need to abstract):

| Area | Linux interface used | Used by |
|---|---|---|
| Process model | fork/exec, setsid, process groups, signals, waitpid, setresuid/gid, setgroups, rlimits | nexusd, BusyBox |
| Resource control | cgroup v2 (`memory.max`, `cpu.max`, `pids.max`, `memory.events`, `cgroup.kill`) | nexusd |
| Confinement | mount namespaces, seccomp-BPF, `PR_SET_NO_NEW_PRIVS` | nexusd sandbox |
| IPC | Unix domain sockets with `SO_PEERCRED`, pipes | nexus CLI ⇄ nexusd |
| Observability | `/proc/{stat,meminfo,loadavg,net/*,<pid>/fd}`, `/sys/class/net`, `/sys/block`, `/sys/bus/pci` | nexus-core resources, diagnostics |
| Storage | ext4 (journaling), squashfs, vfat, block device ioctls, GPT partitions | rcS, installer, updates |
| Networking | TCP/IP v4/v6, packet sockets (DHCP), multicast (mDNS), netfilter/nftables | netd, mdnsd, firewall |
| Devices | devtmpfs, uevents (mdev), watchdog `/dev/watchdog`, ACPI power button input events | init, watchdog, acpid |
| Boot | x86 boot protocol (bzImage), EFI stub, kernel command line | GRUB |

## Possible phases (research, not scheduled)
1. Keep Linux; narrow NEXUS's dependency surface to the table above and
   wrap Linux-specific calls in `nexus-core` modules.
2. Prototype a Rust microkernel under QEMU (e.g. evaluating existing
   projects such as Redox, Theseus, Asterinas for ideas and licensing).
3. A Linux-ABI compatibility layer sufficient for BusyBox + nexusd.
4. Only if 1–3 succeed: driver story for real low-end hardware, which is the
   hardest part and the main reason Linux is used today.
