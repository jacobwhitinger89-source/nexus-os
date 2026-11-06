# Development status

_Last updated: 2026-11-06 — version 0.1.0-dev (pre-alpha)_

## Test results (this commit, dev host: 8-core x86-64, QEMU 11.1 + KVM)

| Suite | Command | Result |
|---|---|---|
| Rust unit tests | `make test-rust` | 28 passed (fmt + clippy `-D warnings` clean) |
| Host supervisor integration | `make test-host` | 23 / 23 |
| QEMU boot (Phase 1 acceptance) | `make test-boot` | 12 / 12 |
| QEMU install + end-to-end | `make test-install` | 48 / 48 |

## Measurements (`make bench`, QEMU/KVM — not physical hardware)

| Metric | Value |
|---|---|
| Kernel (bzImage) | 11.5 MiB |
| System image (squashfs, per slot) | 11.8 MiB |
| Installer ISO | 47.7 MiB |
| Boot to console, live image | 1.5 s |
| Installed: firmware → login prompt (UEFI) | 6.7 s |
| Installation (disk write, after wizard) | 3.1 s |
| RAM used, installed, idle with SSH + nexusd + one app | **35.9 MiB** |
| RAM used, live/installer (rootfs held in RAM) | 70.9 MiB |
| nexusd RSS | 1.1 MiB |
| Each BusyBox daemon RSS | ~1.0–1.2 MiB |

Against the targets in the directive: networking + SSH + full management
environment + a running app fits in ~36 MiB (target 64–128 MiB).

## Roadmap progress

| Phase | State | Verified by |
|---|---|---|
| 0 Architecture & environment | done | ARCHITECTURE.md, Makefile, setup script |
| 1 First bootable image | **done** | boot-test (banner, hardware/memory info, console, clean shutdown) |
| 2 Installation | **done** | install-test: wizard, validation, hidden password, disk confirmation, GPT A/B layout, GRUB UEFI+BIOS |
| 3 Networking & SSH | mostly done | DHCP, Dropbear, host-key fingerprint, key enrollment, finalize, firewall rate limit (tested); **mDNS and static IP not yet verified in tests** |
| 4 Management interface | done | `nexus status/resources/projects`, dashboard (manual check only, no automated TUI test) |
| 5 Project management | done | supervision, restart policies, dependencies, health checks, cgroup limits, logs |
| 6 AI integration | done | `--json` everywhere, diagnostics, agent accounts with forced command, grants, audit |
| 7 Deployment | done | upload/build/atomic switch/health/rollback (host + QEMU tests) |
| 8 Reliability | mostly done | power-loss recovery, A/B update + confirmation, clean shutdown (tested); **automatic fallback after a failing update not yet exercised by a test**; watchdog hang recovery untested |
| 9 Public release | not started | needs physical hardware testing, signed artifacts, CI run on GitHub, repository publication |

## Currently in progress / next objectives
1. Test: failing A/B update falls back to the previous slot (kernel panic image).
2. Test mDNS (`<host>.local`) with two VMs on a shared network; static IP test.
3. `nexus connect` client utility (discover, verify, enroll key, write SSH alias).
4. Signed update bundles / ISO (minisign or similar), then Secure Boot study.
5. Physical hardware matrix (low-end laptops, mini PCs).
6. Optional runtime packages (Node/Python) outside the base image.
7. Publish the repository and run CI on GitHub.

## Failed tests
None at this commit.

## Outstanding bugs
None known. Fixed during this cycle: duplicate shells on the serial console
(setsid under init), daemon respawn loop when DNS was not ready, password
type-ahead echo in the installer, `/dev/null` created 0660 (mdev ran before
`/etc/passwd` was available), `/data` busy at shutdown, CLI reporting
"broken pipe" instead of FORBIDDEN for refused uploads.
