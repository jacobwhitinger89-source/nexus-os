# Known limitations

NEXUS OS is pre-alpha. This list is kept honest and current.

## Hardware & boot
* Tested only in QEMU (q35, KVM; OVMF UEFI and SeaBIOS). No physical
  hardware test yet.
* No Wi-Fi or Bluetooth. Wired Ethernet only, limited driver set
  (documentation/hardware.md).
* Secure Boot is not supported.
* Automatic power-on after AC loss is a firmware setting NEXUS cannot change.
* x86-64 only.

* mDNS and static IPv4 are tested between two VMs on a virtual link, not
  on a real LAN; IPv6 static configuration is not supported.
* The installer/live system keeps its root filesystem in RAM (~71 MiB used);
  installing on machines with 256 MiB works in principle but is untested.
* Automatic fallback after a failing update is tested only for an update
  that panics at boot; an update that boots but never confirms itself
  relies on the same try counter but has no dedicated test.
* `nexus connect` is unit-tested only (fake `ssh`), not yet run against a
  real server in the automated tests. It pins only the ed25519 host key,
  does not detect *hashed* known_hosts entries (it may add a duplicate,
  which OpenSSH tolerates), leaves stale entries for a reinstalled host in
  place (reported; `ssh-keygen -R`), and appends its `Host` block at the end
  of `~/.ssh/config`, so an earlier `Host *` with e.g. `User` wins. No
  Windows support (use the manual steps in installation.md). It does not
  discover servers by itself (no mDNS browsing): pass a name or IP.

## Security
* ISO and update bundles are checksummed (SHA-256) but **not signed**.
* No network isolation between projects (shared network namespace).
* No LSM policy (AppArmor/SELinux/Landlock); no per-project disk quotas.
* Seccomp is a deny-list, not an allow-list.
* sha512-crypt (not yescrypt/argon2) because musl `crypt(3)` supports it.
* No external security audit.

## Deployment & applications
* No language runtimes in the base OS (Node, Python, Go…): ship static
  binaries/self-contained runtimes. An optional runtime package mechanism
  is future work.
* Deploy restarts services (stop old → start new): short downtime per
  deploy; no blue-green/zero-downtime switch yet.
* Health checks connect to 127.0.0.1 only; HTTP checks use HTTP/1.0 without TLS.
* Application metrics only via `$NEXUS_METRICS_FILE` (no HTTP scrape yet).
* Uploads limited to 2 GiB per deployment; one deployment per project at a time.

## System
* `nexus system` hostname/network changes are applied immediately but not
  validated against the live network (a bad static IP can cut SSH access;
  the local console remains).
* Logs are size-bounded (system 4×2 MiB, per service 3×1 MiB) — old lines
  are dropped.
* Time is set by NTP only when the network is up; machines without a working
  RTC start with a wrong clock until NTP succeeds.
* The A/B confirmation only checks that nexusd and Dropbear stay up for 90 s.
