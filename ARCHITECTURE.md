# NEXUS OS architecture

NEXUS OS is a purpose-built server operating system **built from source
components with Buildroot**: the Linux kernel, BusyBox, Dropbear, GRUB and a
handful of libraries, plus NEXUS's own management layer written in Rust. It
is not a modified general-purpose distribution, and it does not claim to have
its own kernel: the kernel is Linux 6.18 LTS (see
[documentation/kernel-roadmap.md](documentation/kernel-roadmap.md) for the
long-term research track).

## Layers

| Layer | Implementation | Location |
|---|---|---|
| A. Boot & kernel | GRUB 2 (UEFI + BIOS), Linux 6.18 LTS, minimised config fragment | `buildroot/board/nexus/linux.fragment`, installer `grub_cfg()` |
| B. Hardware & drivers | storage/NIC/watchdog drivers built in, mdev hotplug + modalias | `linux.fragment`, BusyBox mdev |
| C. System services | BusyBox init + `inittab` respawn, `/usr/libexec/nexus/svc` | `system/rootfs-overlay/etc/inittab` |
| D. Networking | `netd` (udhcpc / static), mdnsd (`<host>.local`), nftables | `usr/libexec/nexus/netd`, `etc/nexus/firewall.nft` |
| E. Authentication & security | Dropbear (keys; password only for bootstrap), sudo for `wheel`, nexusd authorisation + audit | `usr/libexec/nexus/sshd`, `projects/supervisor/src/auth.rs` |
| F. Storage | read-only squashfs system slots A/B + ext4 data partition | installer `install.rs`, `rcS` |
| G. Process & service management | **nexusd** supervisor (cgroup v2, seccomp, namespaces) | `projects/supervisor/` |
| H. Management interface | **nexus** CLI + dashboard | `management/cli/` |
| I. Deployment | nexusd deploy pipeline (upload → build → atomic switch → health → rollback) | `projects/supervisor/src/deploy.rs` |
| J. Monitoring & diagnostics | `nexus-core::resources`, nexusd `diagnose.rs` | `management/core/`, `projects/supervisor/src/diagnose.rs` |

## Boot flow

```
firmware ─► GRUB (ESP, NEXUS_ESP)
              │ grubenv: nexus_slot / nexus_try / nexus_tries
              ▼
           Linux bzImage (/nexus/<slot>/bzImage on the ESP)
              │ root=PARTUUID=<slot> rootfstype=squashfs ro nexus.mode=installed
              ▼
           BusyBox init ─► rcS: mount proc/sys/dev/run/tmp/cgroup2,
              │            e2fsck -p + mount NEXUS_DATA at /data,
              │            seed /data/etc from factory defaults, sysctl
              │            hardening, zram (RAM < 4 GiB), nftables
              ▼
           inittab respawn: syslogd klogd netd ntpd acpid watchdog
                            dropbear mdnsd nexusd   + getty on tty1/ttyS0
              ▼
           nexusd: reap orphans, load projects, autostart, serve socket
```

The installer ISO boots the same root filesystem as an initramfs with
`nexus.mode=live`; `/data` is then a tmpfs and the console runs
`nexus-install`.

## Storage layout (installed)

| # | Label | Size | Content |
|---|---|---|---|
| 1 | BIOSBOOT | 1 MiB | GRUB core for legacy BIOS |
| 2 | NEXUS_ESP | 128 MiB | FAT32: GRUB (UEFI+BIOS), kernels per slot, `grubenv` |
| 3 | NEXUS_A | 1 GiB | squashfs system image (read-only) |
| 4 | NEXUS_B | 1 GiB | squashfs system image (read-only) |
| 5 | NEXUS_DATA | rest | ext4 (journal): `/data` |

The root filesystem is never written at runtime. Mutable identity files
(`/etc/passwd`, `shadow`, `group`, `hostname`, `hosts`, dropbear keys) are
symlinks into `/data/etc`, so a power cut can only affect the journaled data
partition. `/var/log` → `/data/log`, `/home` → `/data/home`.

```
/data
├── etc/            passwd group shadow hostname hosts dropbear/ nexus/
├── log/            messages (syslog, rotated 4×2 MiB), install.log, sudo.log
├── home/<user>/    administrator and agent homes (.ssh/authorized_keys)
└── nexus/
    ├── audit/audit.log                      (root only, JSON lines)
    └── projects/<name>/
        ├── project.toml                     server policy (root 0600)
        ├── current -> releases/<id>          atomic activation symlink
        ├── releases/<id>/                   extracted source + build output
        ├── shared/                          persistent app data ($NEXUS_DATA_DIR)
        ├── home/                            $HOME, build caches
        ├── logs/<service>.log, errors.log   (root only, rotated 3×1 MiB)
        └── deployments/<id>.json, .build.log
```

## nexusd

A single multi-threaded daemon (no async runtime): one monitor thread
(250 ms tick or wake-up), one health thread spawning short-lived check
workers, one accept thread and one thread per client connection, plus two
log-pump threads per running service. All state is in one mutex-protected
structure; handlers only change *desired* state and wake the monitor.

Service lifecycle: `stopped → waiting (dependencies) → starting → running`,
with `backoff` (restart delay doubling 1 s … 60 s), `failed` (crash loop: 10
failures within 5 minutes, or policy `never`), `exited` (clean exit) and
`stopping` (SIGTERM, SIGKILL after `stop_timeout`). Health checks (HTTP, TCP,
command) gate `starting → running`; repeated failures restart the service.

Every project process is started through `sandbox.rs`: new session, cgroup
leaf `/sys/fs/cgroup/nexus/<project>/<service>`, private mount namespace with
its own `/tmp`, `RLIMIT_NOFILE`/`RLIMIT_CORE`, setgroups/setresgid/setresuid
to the project user `nx-<project>`, `PR_SET_NO_NEW_PRIVS`, then a seccomp
deny-list. Project-level limits (memory, CPU, tasks) live on
`/sys/fs/cgroup/nexus/<project>`.

If nexusd itself crashes, init restarts it; on start it kills processes still
in NEXUS cgroups (their log pipes are gone) and restarts projects according
to policy.

## Control protocol

`/run/nexus/nexusd.sock` (mode 0666). JSON request line, JSON reply lines
(`log` events, then one `result`). Caller identity from `SO_PEERCRED`;
authorisation per request (see [SECURITY.md](SECURITY.md)). The `nexus` CLI
is a thin client; all `--json` output follows
[documentation/json-schema.md](documentation/json-schema.md).

## Interfaces a future custom kernel would have to provide

See [documentation/kernel-roadmap.md](documentation/kernel-roadmap.md).
