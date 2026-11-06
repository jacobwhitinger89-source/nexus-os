# nexus CLI reference

Run `nexus help` on the server for the authoritative list. All commands
accept `--json` (see [json-schema.md](json-schema.md)).

| Command | Permission | Description |
|---|---|---|
| `nexus status` | any | server overview (resources + projects visible to you) |
| `nexus resources [--health]` | any | CPU, memory, storage, disks (SMART with `--health`), network, GPU |
| `nexus dashboard` | any | interactive dashboard (default on admin SSH login) |
| `nexus projects` | read | projects you can read |
| `nexus project status <p>` | read | full status |
| `nexus project create <p> [--memory MiB] [--cpu PCT] [--pids N]` | admin | create project + service user |
| `nexus project limits <p> …` | admin | change limits (live) |
| `nexus project delete <p> --confirm <p>` | admin | delete project, releases and data |
| `nexus start/stop/restart <p> [--service S]` | restart | control services |
| `nexus logs <p> [--service S] [--errors] [--lines N]` | logs | merged or per-service logs |
| `nexus doctor <p>` | read | diagnostics |
| `nexus deploy <p> [--source DIR\|-] [--message M]` | upload+deploy (+build) | deploy from stdin tar or a directory |
| `nexus history <p>` | read | deployment history |
| `nexus rollback <p> [--to ID]` | deploy | activate an earlier release |
| `nexus env list\|set\|unset <p> [K[=V]]` | config | server-side secrets (values never shown) |
| `nexus agent add\|remove\|list` | admin | restricted agent accounts |
| `nexus access grant\|revoke\|list` | admin | project permissions |
| `nexus audit [--lines N]` | admin | audit trail |
| `nexus whoami` | any | identity and grants |
| `nexus ssh status\|add-key\|finalize\|allow-passwords` | — / own account / admin | SSH policy |
| `nexus system hostname\|network\|update\|slot\|confirm-boot\|reboot\|poweroff` | root (sudo) | host configuration, A/B updates |
