# Development status

_Last updated: 2026-11-06 — version 0.1.0-dev (pre-alpha)_

## Test results (this commit, dev host: 8-core x86-64, QEMU 11.1 + KVM)

| Suite | Command | Result |
|---|---|---|
| Rust unit tests | `make test-rust` | 39 passed (fmt + clippy `-D warnings` clean) |
| Host supervisor integration | `make test-host` | 23 / 23 |
| QEMU boot (Phase 1 acceptance) | `make test-boot` | 12 / 12 |
| QEMU install + end-to-end | `make test-install` | 53 / 53 |
| QEMU two-machine networking | `make test-net` | 17 / 17 |

## Measurements (`make bench`, QEMU/KVM — not physical hardware)

| Metric | Value |
|---|---|
| Kernel (bzImage) | 11.5 MiB |
| System image (squashfs, per slot) | 11.8 MiB |
| Installer ISO | 47.7 MiB |
| Boot to console, live image | 1.2 s |
| Installed: firmware → login prompt (UEFI) | 5.2 s |
| Installation (disk write, after wizard) | 1.9 s |
| RAM used, installed, idle with SSH + nexusd + one app | **37.5 MiB** |
| RAM used, live/installer (rootfs held in RAM) | 77.5 MiB (was 70.9; not yet investigated) |
| nexusd RSS | 1.1 MiB |
| Each BusyBox daemon RSS | ~1.0–1.2 MiB |

Against the targets in the directive: networking + SSH + full management
environment + a running app fits in ~38 MiB (target 64–128 MiB).

## Roadmap progress

| Phase | State | Verified by |
|---|---|---|
| 0 Architecture & environment | done | ARCHITECTURE.md, Makefile, setup script |
| 1 First bootable image | **done** | boot-test (banner, hardware/memory info, console, clean shutdown) |
| 2 Installation | **done** | install-test: wizard, validation, hidden password, disk confirmation, GPT A/B layout, GRUB UEFI+BIOS |
| 3 Networking & SSH | done | DHCP, Dropbear, host-key fingerprint, key enrollment, finalize, firewall rate limit, static IPv4 + persistence, mDNS discovery/resolution and hostname change (two-VM test); `nexus connect` client (fingerprint check, key enrollment, alias — QEMU test) |
| 4 Management interface | done | `nexus status/resources/projects`, dashboard (manual check only, no automated TUI test) |
| 5 Project management | done | supervision, restart policies, dependencies, health checks, cgroup limits, logs |
| 6 AI integration | done | `--json` everywhere, diagnostics, agent accounts with forced command, grants, audit |
| 7 Deployment | done | upload/build/atomic switch/health/rollback (host + QEMU tests) |
| 8 Reliability | mostly done | power-loss recovery (with durable state writes), A/B update + confirmation, automatic fallback after a failing update (3 panicking trial boots), clean shutdown (tested); watchdog hang recovery untested |
| 9 Public release | not started | needs physical hardware testing, signed artifacts, CI run on GitHub, repository publication |

## Currently in progress / next objectives
1. Stronger power-loss test: reset immediately after each kind of state change.
2. Watchdog hang-recovery test.
3. Investigate live-image RAM growth (70.9 → 77.5 MiB).
4. Signed update bundles / ISO (minisign or similar), then Secure Boot study.
5. Physical hardware matrix (low-end laptops, mini PCs).
6. Optional runtime packages (Node/Python) outside the base image.
7. Publish the repository and run CI on GitHub.

## Failed tests
None at this commit.

## Outstanding bugs
Intermittent: one boot-test run in 12 lost a console command (no echo at
all) while the host was under heavy load; not reproduced in 11 reruns.

Fixed during this cycle: state files lost on power cut (no fsync),
mdnsd renaming itself to `<host>-2` after a false conflict, Dropbear
restart killing the requesting session, duplicate shells on the serial console
(setsid under init), daemon respawn loop when DNS was not ready, password
type-ahead echo in the installer, `/dev/null` created 0660 (mdev ran before
`/etc/passwd` was available), `/data` busy at shutdown, CLI reporting
"broken pipe" instead of FORBIDDEN for refused uploads.
