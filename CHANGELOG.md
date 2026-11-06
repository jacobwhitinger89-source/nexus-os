# Changelog

## 0.1.0-dev (unreleased)
First working system.
* Buildroot 2026.02.3 LTS external tree: Linux 6.18.60 LTS, BusyBox 1.37,
  Dropbear, GRUB 2.12 (UEFI + BIOS), musl toolchain; hybrid installer ISO.
* Read-only squashfs A/B system slots, ext4 data partition, GRUB boot
  counting, `nexus system update` with trial boot and automatic confirmation.
* Installer wizard (hardware/network check, hostname, admin, hidden
  password, disk confirmation by name), unattended `--answers` mode.
* nexusd supervisor: per-project users, cgroup v2 limits, seccomp, private
  /tmp, health checks, restart policies with crash-loop detection,
  dependencies, log capture/rotation, structured diagnostics, audit log.
* Deployments: streamed tar upload, sandboxed builds, atomic activation,
  health verification, automatic rollback, history.
* nexus CLI with versioned JSON output, dashboard, agent accounts with
  forced command and project-scoped permissions, SSH key enrollment and
  finalize, firewall with SSH rate limiting.
* Test suites: unit, host integration, QEMU boot and end-to-end install.
