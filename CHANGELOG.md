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
* `nexus connect` (also standalone `nexus-connect` with no Linux-only code, for macOS):
  client-side onboarding — fetches the ed25519 host key, verifies its
  SHA256 fingerprint (interactive or `--fingerprint`), pins it in
  known_hosts, enrolls a public key via `nexus ssh add-key`, writes an
  idempotent marked `~/.ssh/config` alias block with ControlMaster.
* Test suites: unit, host integration, QEMU boot and end-to-end install.
* Server hardware: built-in drivers for Intel 10/25/40/100G (ixgbe, i40e,
  ice), Broadcom bnxt, Mellanox mlx5, Aquantia AQtion, vmxnet3 Ethernet and
  MegaRAID, mpt3sas, hpsa, SmartPQI, PVSCSI storage controllers; Realtek
  rtl_nic firmware (r8169/RTL8125, USB RTL8152/8153) included.
* `make test-usb`: the ISO as a USB stick boots and installs under UEFI and
  BIOS; the installed disk boots. Runs in CI with the network test.
* Fixed: the installer could not find its media on a USB stick (only on a
  CD): blkid's device scan skips whole disks that carry a partition table,
  as a hybrid ISO written with `dd` does. Whole disks are now probed
  directly, and the installer waits up to 30 s for a slow stick to appear.
* `scripts/publish-github.sh` (documentation/publishing.md): checked,
  confirmed publication to a public GitHub repository, optional release tag.

### Fixed (since the first bootable image)
* **Power-loss durability:** state on /data (SSH `authorized_keys`, project
  policies, grants, deployment records, network/hostname/SSH config) is now
  written with fsync + directory fsync (`nexus_core::fsutil`), and a
  release's files are flushed before `current` switches to it. Before, a
  power cut within ~30 s of a change could leave these files empty (seen as
  an admin locked out and a project skipped after reset). The audit log is
  synced per entry; the update kernel is synced before the trial boot is
  scheduled.
* **mDNS name conflicts:** mdnsd started before the interface had its
  addresses, re-probed when they appeared, took its own announcement for
  another host and renamed itself (`pixel-2.local`). It now starts once the
  interface is addressed and is restarted cleanly when the IPv4 address
  changes (DHCP hook, static configuration).
* `nexus ssh finalize` and other intentional daemon restarts are no longer
  delayed 10 s by the respawn throttle.
* SSH rate limit raised to 60 new connections/minute (burst 40).
* `mquery` included for mDNS diagnostics.

### Tests
* `make test-net`: two installed machines on one virtual link — unattended
  install, static IPv4, persistence across reboot, mDNS discovery and
  resolution, hostname change.
* install-test: a broken update (valid checksums, unbootable image) falls
  back to the good slot after three trial boots.
* Restarting Dropbear (`nexus ssh finalize`, `allow-passwords`) could kill
  the SSH session that requested it: sessions re-parented to init during
  the /proc scan were signalled too. Dropbear now writes
  `/run/nexus/dropbear.pid` and only that process is restarted.
* `nexus connect --ssh-dir` writes `UserKnownHostsFile` and `ControlPath`
  for that directory into the alias (previously pointed at ~/.ssh).
* install-test: `nexus connect` against the installed server (wrong
  fingerprint refused, key enrolled with the password, alias login); the
  installer fingerprint is now captured and compared in full (a streaming
  match could capture just a prefix and pass trivially).
