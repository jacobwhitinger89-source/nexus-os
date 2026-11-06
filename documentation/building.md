# Building NEXUS OS

Host: Linux x86-64 with ~25 GB free disk, 4+ GB RAM. Nothing is installed
system-wide and no host disk is touched; all output stays in the repository
(`build/`, `dl/`, `out/`, `.tools/`) and Rust in `~/.cargo`.

```sh
make setup    # checks host packages, builds bc / shims wget if missing,
              # installs Rust via rustup (user-local), fetches + verifies Buildroot
make build    # Rust components (static musl) + full Buildroot image (~30–60 min first time)
make run      # boot the live image in QEMU on the serial console (Ctrl-A X quits)
make test     # cargo fmt/clippy/tests + host supervisor test + QEMU boot + full install test
```

Artifacts in `build/output/images/`:

| file | purpose |
|---|---|
| `nexus-<ver>-x86_64.iso` | hybrid BIOS/UEFI installer (CD or USB) |
| `nexus-<ver>-x86_64-update.tar` | A/B update bundle |
| `bzImage`, `rootfs.cpio.zst` | live system (QEMU `-kernel/-initrd`) |
| `rootfs.squashfs` | installed system slot image |
| `SHA256SUMS` | checksums |

Other targets: `make run-iso` (installer in QEMU with a blank 8 GiB virtual
disk under `build/qemu/`), `scripts/qemu-run.sh disk|disk-bios`,
`make menuconfig`, `make linux-menuconfig`, `make savedefconfig`,
`make bench`.

## Layout
```
buildroot/      BR2_EXTERNAL tree: defconfig, kernel/BusyBox fragments,
                post-build/post-image scripts, nexus-tools package, hashes
system/         root filesystem overlay (init, services, firewall, defaults)
management/     nexus-core (library) and nexus CLI/dashboard
projects/       nexusd supervisor (supervision, deploy, diagnostics, auth)
installer/      nexus-install wizard
examples/       hello-http sample application
tests/          host and QEMU integration tests
benchmarks/     memory/boot/footprint measurements
scripts/        host setup, QEMU runner, shims
documentation/  user and developer documentation
```

## Reproducibility
`BR2_REPRODUCIBLE=y`, pinned Buildroot (SHA-256 + GPG verified), forced
hash checks for every download, pinned kernel hash, `Cargo.lock` with
`--locked`. Remaining sources of variance are documented in
KNOWN_LIMITATIONS.md.
