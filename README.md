# NEXUS OS

**A lightweight, installable server operating system that turns low-end
laptops, mini PCs and old desktops into reliable application hosts —
managed over SSH by humans and AI coding agents alike.**

> Status: **pre-alpha (0.1.0-dev)**. Built and tested in QEMU; not yet tested
> on physical hardware. See [DEVELOPMENT_STATUS.md](DEVELOPMENT_STATUS.md)
> and [KNOWN_LIMITATIONS.md](KNOWN_LIMITATIONS.md). "NEXUS" is a provisional
> name ([documentation/naming.md](documentation/naming.md)).

## What it is
* A minimal Linux-based system **built from components with Buildroot**
  (Linux 6.18 LTS, BusyBox, Dropbear, GRUB, musl) — not a modified
  distribution. No desktop, no cloud account, no external auth service.
* A **read-only, A/B-updatable system image** plus one journaled data
  partition: power cuts cannot corrupt the OS, failed updates fall back.
* **nexusd**, a project supervisor written in Rust: per-project users,
  cgroup v2 limits, seccomp, health checks, restart policies, dependency
  ordering, log capture, structured diagnostics.
* **nexus**, a CLI and terminal dashboard with stable `--json` output so AI
  agents can inspect, deploy and troubleshoot through SSH.
* **Deployments** that build, activate atomically, health-check and roll back
  automatically.

```
=================================================
                    NEXUS OS
=================================================

 HOSTNAME: pixel
 STATUS: ONLINE
 UPTIME: 14d 06h

 SYSTEM RESOURCES
-------------------------------------------------
 CPU:       4% USED (4 cores, load 0.08)
 RAM:       21% USED (412.0 MB of 1.9 GB)
 DISK:      3% USED (221.4 GB free on /data)
 ...
```

## Quick start
```sh
# on the server: install from the ISO (documentation/installation.md), then
ssh-copy-id admin@pixel.local
ssh admin@pixel.local nexus ssh finalize          # keys only from now on
ssh admin@pixel.local nexus project create hello

# from your project directory (contains nexus.toml)
tar -czf - . | ssh admin@pixel.local nexus deploy hello
ssh admin@pixel.local nexus project status hello
```

## Documentation
| | |
|---|---|
| Install | [documentation/installation.md](documentation/installation.md) |
| Build from source | [documentation/building.md](documentation/building.md) |
| Deploying applications / `nexus.toml` | [documentation/app-integration.md](documentation/app-integration.md) |
| AI agents | [documentation/agents.md](documentation/agents.md) |
| CLI | [documentation/cli.md](documentation/cli.md) · JSON: [documentation/json-schema.md](documentation/json-schema.md) |
| Architecture | [ARCHITECTURE.md](ARCHITECTURE.md) |
| Security | [SECURITY.md](SECURITY.md) |
| Updates (A/B) | [documentation/updates.md](documentation/updates.md) |
| Hardware | [documentation/hardware.md](documentation/hardware.md) |
| Troubleshooting | [documentation/troubleshooting.md](documentation/troubleshooting.md) |
| Releases | [documentation/releasing.md](documentation/releasing.md) |
| Kernel research track | [documentation/kernel-roadmap.md](documentation/kernel-roadmap.md) |

## Building
```sh
make setup && make build && make run    # see documentation/building.md
make test                               # unit, host integration, QEMU boot + install tests
```

## License
Apache License 2.0 — see [LICENSE](LICENSE). Bundled third-party components
(Linux, BusyBox, GRUB, Dropbear, …) keep their own licenses; Buildroot's
`make legal-info` lists them.
