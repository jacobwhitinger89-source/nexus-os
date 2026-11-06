# Machine-readable output (schema version 1)

Every `nexus … --json` command prints exactly one JSON document on stdout.
Top-level documents carry:

| field | meaning |
|---|---|
| `schema_version` | integer, currently `1`. Fields may be **added** within a version; never renamed or removed. |
| `kind` | document type, e.g. `status`, `projects`, `logs`, `doctor`, `deploy`, `error` |
| `generated_at` | RFC 3339 UTC timestamp |

`project status` returns a `ProjectStatus` object directly (it has
`schema_version` but no `kind`). `resources` returns a `Resources` object.

## Exit codes
`0` success · `1` operation failed · `2` usage error · `3` permission denied
(`FORBIDDEN`) · `4` not found · `5` supervisor unreachable.

## Errors
```json
{ "schema_version": 1, "kind": "error", "generated_at": "…",
  "error": { "code": "FORBIDDEN", "message": "user agent-claude lacks the \"deploy\" permission on project web", "details": null } }
```
Error codes: `FORBIDDEN`, `NOT_FOUND`, `INVALID`, `CONFLICT`,
`CONFIRMATION_REQUIRED`, `NOT_VERIFIED`, `USAGE`, `FAILED`,
`SUPERVISOR_UNREACHABLE`, plus any diagnostic code below (deployments).

## ProjectStatus (`nexus project status <p> --json`)
```json
{
  "schema_version": 1,
  "project": "flight-efb",
  "id": "prj-20261106t120000z",
  "status": "active",            // active | degraded | inactive | failed | deploying | not_deployed
  "release": { "id": "20261106T121500Z-3fa1", "deployed_at": "…", "deployed_by": "admin" },
  "services": [{
    "name": "api-server",
    "state": "running",          // stopped | waiting | starting | running | stopping | backoff | failed | exited
    "health": "healthy",         // none | starting | healthy | unhealthy
    "pid": 812, "uptime_seconds": 3600, "restarts": 0,
    "last_exit": { "code": 1, "signal": null, "at": "…" },   // or null
    "memory_bytes": 18350080,    // null when not measurable
    "ports": [{ "port": 3000, "protocol": "tcp", "public": true, "listening": true }],
    "depends_on": ["database"]
  }],
  "failure": null,               // most severe current Diagnostic, or null
  "resources": { "memory_bytes": 19000000, "memory_limit_bytes": 536870912,
                 "cpu_percent": 1.5, "pids": 7, "oom_kills": 0 },
  "metrics": { "api-server": { "metrics": { "http_requests": 1482 } } },  // or null
  "diagnostics_available": true,
  "logs_available": true
}
```
`cpu_percent` is percent of one core over the interval since the previous
status request for that project (null on the first request).

## Diagnostic
```json
{
  "code": "PORT_IN_USE",
  "severity": "error",           // info | warning | error | critical
  "project": "flight-efb",
  "service": "api-server",
  "message": "The requested port 3000 is already occupied.",
  "timestamp": "2026-11-06T12:00:00Z",
  "details": { "exit": { "exit_code": 1, "signal": null, "ran_for_seconds": 0 },
               "conflict": { "port": 3000, "owner_pid": 455, "owner_command": "node", "owner_project": "api-backend" } },
  "logs": ["2026-11-06T12:00:00Z err Error: listen EADDRINUSE :::3000"],
  "suggested_actions": ["Stop the process holding port 3000, or change the service's port in nexus.toml."],
  "recovery_actions": [
    { "action": "view_logs", "command": "nexus logs flight-efb --service api-server", "permission": "logs" },
    { "action": "restart",   "command": "nexus restart flight-efb --service api-server", "permission": "restart" },
    { "action": "rollback",  "command": "nexus rollback flight-efb", "permission": "deploy" }
  ]
}
```

### Diagnostic codes
| code | meaning |
|---|---|
| `PORT_IN_USE` | a declared port is held by another process (`details.conflict`) |
| `EXEC_NOT_FOUND` | command or working directory missing (exit 127) |
| `PERMISSION_DENIED` | permission error (exit 126, EACCES in log, port < 1024, …) |
| `OUT_OF_MEMORY` | killed by the OOM killer at the project memory limit |
| `PIDS_LIMIT` | fork failed at the project task limit |
| `DISK_FULL` | data partition below 1 % free or ENOSPC in log |
| `MEMORY_PRESSURE` | (detail flag) system had < 5 % memory available |
| `EXITED_NONZERO` / `KILLED_BY_SIGNAL` | generic exit; read `logs` |
| `CRASH_LOOP` | 10 failures in 5 minutes; restarts stopped until `nexus start` |
| `HEALTH_CHECK_FAILED` | health check failing after `retries` |
| `DEPENDENCY_FAILED` | waiting on a dependency that is not running |
| `MANIFEST_INVALID` | `nexus.toml` missing or invalid |
| `NO_RELEASE` | nothing deployed yet |
| `UPLOAD_FAILED` / `BUILD_FAILED` / `DEPLOY_HEALTH_FAILED` / `SPAWN_FAILED` | deployment stages |

## Other documents
* `projects`: `{ "projects": [ { "name", "id", "status", "services_total", "services_running", "release" } ] }`
* `logs`: `{ "project", "service", "errors_only", "lines": ["<RFC3339> <out|err|nexus> [<service>] text", …] }`
* `doctor`: `{ "project", "status", "healthy": bool, "checks": [{ "check", "status": "pass|warn|fail", "message" }], "diagnostics": [Diagnostic], "recent_errors": [line] }`
* `deploy`: `{ "deployment": DeploymentRecord }`; on failure the error's
  `details` holds `{ "deployment": DeploymentRecord, "rolled_back": bool }`.
* `deploy_history`: `{ "project", "deployments": [DeploymentRecord] }` newest first.
* DeploymentRecord: `{ "id", "project", "status": "uploading|building|activating|succeeded|failed|rolled_back", "started_at", "finished_at", "deployed_by", "message", "previous_release", "source_bytes", "error": Diagnostic|null }`
* `status`: `{ "hostname", "os_version", "kernel", "uptime_seconds", "boot_mode", "boot_slot", "data_storage", "supervisor": { "version", "cgroups", "isolation": "full|limited" }, "ssh": { "password_auth", "host_keys": [{ "type", "fingerprint" }] }, "projects": [ProjectSummary], "status": "online|degraded", "supervisor_reachable", "resources": Resources }`
* Resources (`nexus resources --json`): `hostname, os_version, kernel, uptime_seconds, cpu{model, cores, usage_percent, per_core_percent[], available_percent, load_average[3]}, memory{total_bytes, used_bytes, available_bytes, cached_bytes, used_percent, swap{configured,total_bytes,used_bytes}}, storage[{mount, device, filesystem, total_bytes, used_bytes, available_bytes, used_percent, read_only}], disks[{device, model, size_bytes, health: PASSED|FAILED|NOT SUPPORTED|NOT CHECKED, temperature_celsius}], network[{name, state, mac, ipv4[], ipv6[], speed_mbps, rx_bytes_per_sec, tx_bytes_per_sec, rx_bytes_total, tx_bytes_total}], connections{tcp_established, tcp_listening}, gpus[{pci_address, vendor, vendor_id, device_id, driver, utilization_percent, memory_used_bytes, memory_total_bytes, metrics: SUPPORTED|NOT SUPPORTED}]`.
  Unmeasurable values are `null`; an empty `gpus` array means no GPU detected.
