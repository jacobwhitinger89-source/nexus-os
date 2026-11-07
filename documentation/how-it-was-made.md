# How NEXUS OS is made

NEXUS OS is not a modified distribution. It is assembled from source with
**Buildroot**, which downloads each component, cross-compiles it with a
pinned toolchain and packs the result into a small read-only image. On top
of that base sit NEXUS's own init scripts and a management stack written in
Rust.

## What runs on the server
| Component | Version | Role |
|---|---|---|
| Linux kernel | 6.18.60 LTS (`-nexus`, x86_64_defconfig + `buildroot/board/nexus/linux.fragment`) | drivers built in, no module loading needed |
| musl libc | 1.2.5 | C library (static Rust binaries also use musl) |
| BusyBox | 1.37.0 | init, shell, core utilities, udhcpc, ntpd, mdev |
| Dropbear | 2026.91 | SSH server |
| GRUB | 2.12 | bootloader, UEFI and legacy BIOS, A/B slot selection |
| nftables | 1.1.4 | firewall |
| mdnsd | 0.12 | `<hostname>.local` announcement |
| util-linux | 2.41.4 | blkid, partitioning helpers |
| e2fsprogs | 1.47.3 | ext4 for `/data` |
| linux-firmware | Buildroot 2026.02.3 | Realtek Ethernet firmware only |
| nexusd, nexus, nexus-install | 0.1.0-dev | NEXUS's own code (Rust, this repository) |

Root filesystem: squashfs (zstd), read-only, two slots (A/B). State lives
on an ext4 `/data` partition.

## Build tools
| Tool | Version |
|---|---|
| Buildroot | 2026.02.3 LTS (`BR2_EXTERNAL` tree in `buildroot/`) |
| Cross toolchain | Bootlin x86-64 musl stable 2025.08-1 (GCC 14.3.0) |
| Rust | 1.99.1 stable, edition 2024, target `x86_64-unknown-linux-musl` |
| ISO | xorriso (hybrid: CD, USB stick, UEFI and BIOS) |
| Tests | Python 3 + QEMU (q35, KVM, OVMF/SeaBIOS) |

## Development host
Written and tested on an x86-64 laptop running Omarchy (Arch-based, Linux
7.2.8), with host GCC 16.2.1, Python 3.14 and QEMU 11.1.2. Any recent
Linux distribution with the packages in [building.md](building.md) can
build it; CI uses Ubuntu 24.04.

The code was written with AI coding agents (Claude Code), working from a
written design brief and checked by the automated test suites in `tests/`.

## Size of the code
About 11,500 lines of Rust (supervisor, CLI, installer, client tool) and
about 1,700 lines of shell and Python (init scripts, build scripts, tests).
