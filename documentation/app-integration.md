# Integrating an application with NEXUS

A deployable project is a directory (uploaded as a tar archive) containing
a `nexus.toml` manifest at its root.

```toml
[project]
name = "flight-efb"             # optional; must match the server project if set

[build]                          # optional; omit to ship prebuilt artifacts
command = "cargo build --release"
timeout = 1800                   # seconds

[deploy]
health_timeout = 60              # seconds for all services to become healthy
keep_releases = 3                # old releases kept on disk

[[service]]
name = "database"
command = "./bin/db --port 5432"
ports = [{ port = 5432, public = false }]   # loopback-only service
health = { tcp = 5432 }

[[service]]
name = "api-server"
command = "./target/release/api"            # run via /bin/sh -c "exec …" in the release dir
workdir = "."                               # relative to the release
depends_on = ["database"]                   # started after database is running/healthy
restart = "on-failure"                      # never | on-failure | always
restart_delay = 1                           # seconds, doubles per consecutive failure (max 60)
stop_timeout = 10                           # SIGTERM → SIGKILL
ports = [3000]                              # bare numbers are public (firewall opened while running)
env = { RUST_LOG = "info" }

[service.health]
http = "/health"                 # or: tcp = 3000, or: command = "./check.sh"
port = 3000                      # defaults to the first port
interval = 10
timeout = 3
retries = 3
start_period = 30                # failures ignored while starting
```

Unknown keys are rejected (typos fail the deployment with
`MANIFEST_INVALID` instead of being ignored).

## Runtime environment
Services run as the project user `nx-<project>` with:

| variable | value |
|---|---|
| `PORT` | first declared port |
| `NEXUS_PROJECT`, `NEXUS_SERVICE`, `NEXUS_RELEASE` | identifiers |
| `NEXUS_RELEASE_DIR` | the release directory (treat as read-only) |
| `NEXUS_DATA_DIR` | persistent storage that survives deployments |
| `NEXUS_METRICS_FILE` | where to publish metrics (below) |
| `HOME` | project home (build caches) |
| your `env` table, then server-side `nexus env set` values (override) | |

Bind to `0.0.0.0` (or `127.0.0.1` for private ports). Ports below 1024 are not
available to unprivileged services. Log to stdout/stderr; NEXUS captures,
timestamps and rotates them. Write data only below `$NEXUS_DATA_DIR`.

## Application metrics
NEXUS never invents application numbers. To show metrics in `nexus project
status` and the dashboard, write a JSON object to `$NEXUS_METRICS_FILE`
(write a temp file and rename it for atomicity):

```json
{ "metrics": { "http_requests": 1482, "active_connections": 12, "errors": 2 } }
```

Values may be numbers or strings; keys are shown upper-cased with `_` as
spaces. Files above 64 KiB are ignored. See `examples/hello-http`.

## Deploying
```sh
# from your machine, over SSH (agents: same command)
tar -czf - . | ssh admin@pixel.local nexus deploy flight-efb --message "fix login"
```
Pipeline: upload → extract as the project user → validate `nexus.toml` →
build (sandboxed, output streamed) → stop old services → atomic switch of
`current` → start → wait for health → success, or automatic rollback to the
previous release. The previous working release is never deleted before the
new one passes its health checks.

Weak servers: build on your workstation (e.g. a static musl binary) and
upload the artifact without a `[build]` section. Language runtimes (Node,
Python, Go…) are not part of the base OS; ship a self-contained runtime in
the release or use compiled artifacts (see KNOWN_LIMITATIONS.md).
