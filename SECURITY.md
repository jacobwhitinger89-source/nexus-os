# Security

## Reporting a vulnerability

Please report security problems privately via GitHub Security Advisories on
the project repository (or the maintainers' contact in the repository
profile). Do not open public issues for unfixed vulnerabilities. NEXUS OS is
**alpha software** and has not had an external security review.

## Security model

### Trust boundaries

| Principal | How it authenticates | What it can do |
|---|---|---|
| Physical console user | local login (password) | everything an administrator can; recovery path when SSH is locked down |
| Administrator (`wheel`) | SSH key (password only during bootstrap) | all nexus operations; root via `sudo` (password required) |
| Agent account (`agent-<name>`) | SSH key with forced command | only `nexus …` commands, only on projects/permissions granted |
| Project identity (`nx-<project>`) | none (no password, `/bin/false` shell) | runs that project's code; owns its release/shared/home dirs |
| Network | — | SSH (rate limited), mDNS, ICMP, and ports declared public by running services |

### Passwords
* Hashed with SHA-512-crypt, 100 000 rounds, 96-bit random salt
  (RustCrypto `sha-crypt`); format `$6$rounds=…` is verified by musl
  `crypt(3)` used by BusyBox `login` and Dropbear. Plain text is never
  written to disk or logs; the installer reads it with echo disabled.
  (yescrypt/argon2 are stronger but not supported by musl `crypt`.)
* Root has no password and cannot log in; administrators use `sudo`.

### SSH
* Dropbear with ed25519 and ECDSA host keys generated at install time. The
  installer prints the SHA256 fingerprints and explains how to compare them.
* Password authentication is enabled **only** for initial enrollment
  (`PASSWORD_AUTH=yes` in `/data/etc/nexus/ssh.conf`). `nexus ssh finalize`
  disables it, but only after the system log shows a successful public-key
  login by that administrator, to prevent lock-out. Local console login
  always keeps working.
* Root logins (`-w`) and remote port forwarding (`-k`) are disabled; at
  most 5 authentication attempts per connection (`-T 5`).
* nftables limits new SSH connections to 12/minute per source address
  (burst 6); excess attempts are dropped and logged (`nexus-fw
  ssh-ratelimit`). Dropbear logs failed logins to syslog; the dashboard's
  Security screen shows them.
* Private keys are never transmitted: `nexus ssh add-key` and `nexus agent
  add` accept public key lines only and refuse anything containing
  `PRIVATE KEY`.

### Authorisation (nexusd)
* Caller identity comes from the kernel (`SO_PEERCRED`), not from anything
  the client sends.
* Administrators: uid 0 and members of `wheel`.
* Everyone else needs grants in `/data/etc/nexus/access.json`:
  `read, logs, upload, build, deploy, restart, config` per project (or `*`).
  Deploying needs `upload` + `deploy`; deploying a release whose
  `nexus.toml` has a `[build]` step additionally needs `build`.
* Destructive operations (project delete, agent removal) require the
  administrator role **and** an explicit confirmation argument repeating the
  name. Access management, limits and the audit log are administrator-only.
* Server-side environment variables (secrets) are write-only through the
  API: `env list` returns names, never values.
* Every mutating request and every denial is appended to
  `/data/nexus/audit/audit.log` (JSON lines, root-only).

### Application isolation (tested — see tests/qemu/install-test.py)
Each project runs as its own unprivileged user with: separate cgroup v2
group (memory, CPU, task limits; whole-service OOM kill), private mount
namespace and `/tmp`, `no_new_privs`, a seccomp filter denying kernel
attack surface not needed by servers (module loading, kexec, bpf,
perf_event_open, userfaultfd, keyrings, mount APIs, namespaces, ptrace,
io_uring, clock changes, reboot, …; non-x86-64 syscall ABIs are killed),
no core dumps, bounded open files. User namespaces are disabled system-wide
(`user.max_user_namespaces=0`). Project directories are not readable by
other projects.

**Not provided (yet):** network isolation between projects (all share the
host network namespace and can reach each other's loopback ports), a
read-only view of other system paths beyond normal permissions, disk quotas,
LSM (AppArmor/SELinux/Landlock) policies. Do not host mutually hostile
tenants on one NEXUS server.

### System integrity
* The system image is a read-only squashfs; A/B slots let a bad update
  fall back automatically. Installer payload and update bundles are
  verified with SHA-256 checksums.
* **Limitation:** update bundles and ISOs are not cryptographically
  *signed* yet; checksums detect corruption, not tampering. Only
  administrators (root) can apply updates. Secure Boot is not supported.
* Kernel hardening: `STRICT_DEVMEM`, `dmesg_restrict`, Yama ptrace scope 1,
  `kptr_restrict=2`, hardened usercopy, zero-init allocations, unprivileged
  BPF disabled, `LEGACY_TIOCSTI` off, protected links/fifos/regular files.
  Userspace built with `-fstack-protector-strong`, full RELRO,
  `_FORTIFY_SOURCE=2`. Rust components are statically linked (musl).

### Build supply chain
* Buildroot release tarball pinned by SHA-256 (and its GPG signature was
  verified when first imported); every package download is checked against
  Buildroot's hashes (`BR2_DOWNLOAD_FORCE_CHECK_HASHES`); the Linux tarball
  hash comes from kernel.org's signed `sha256sums.asc`.
* Rust dependencies are locked by `Cargo.lock` and built with `--locked`.

See [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md) for the full list of gaps.
