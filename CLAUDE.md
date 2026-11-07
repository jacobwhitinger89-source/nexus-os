# Notes for AI coding agents working on this repository

* Build: `make build` (Rust + Buildroot, long); quick loop: `cargo build`,
  `make test-rust`, `make test-host` (runs real nexusd unprivileged against a
  temp root via `NEXUS_ROOT`/`NEXUS_SOCKET`/`NEXUS_CGROUP_ROOT`).
* System tests run only in QEMU (`make test-boot`, `make test-install`).
  Never write to host block devices; never run the installer on the host.
* Layout: `management/core` (shared lib: paths, schema, manifest, resources,
  ipc, term), `management/cli` (nexus), `management/connect` (client-side
  `nexus connect`; no nexus-core dependency so it builds off-Linux),
  `projects/supervisor` (nexusd), `installer` (nexus-install), `system/rootfs-overlay` (init scripts),
  `buildroot/` (external tree). After changing Rust code used in the image,
  `make build` re-packages it (`nexus-tools` package from `out/rust-bin`).
* Invariants: root filesystem is read-only on installed systems (state goes
  to `/data`); never log secrets; JSON schema changes are additive; every
  privileged nexusd operation goes through `auth.rs` checks and the audit log;
  previous release is never deleted before the new one is healthy.
* Keep DEVELOPMENT_STATUS.md, KNOWN_LIMITATIONS.md and CHANGELOG.md current.
* Publishing to GitHub: `scripts/publish-github.sh` (see
  documentation/publishing.md). Public and outward-facing: confirm with the
  user before running it; never pass `--yes` on your own.
