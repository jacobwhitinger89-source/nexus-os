# Using NEXUS from AI coding agents

NEXUS is designed so an agent (Claude Code, Codex, any SSH-capable tool) can
inspect, deploy and troubleshoot without screen-scraping.

## 1. Give the agent its own restricted account (administrator, once)
```sh
nexus agent add claude --key 'ssh-ed25519 AAAA… claude@laptop'
nexus access grant agent-claude flight-efb read,logs,upload,deploy,restart
```
The key is installed with a forced command: it can run `nexus …` and
nothing else (no shell, no port forwarding, no PTY). Grants are per project;
use `build` as well if the agent may run build commands on the server, and
`config` for `nexus env`. Every action and every denial is audited
(`nexus audit`).

## 2. Agent workflow
```sh
H=agent-claude@pixel.local
ssh $H nexus projects --json
ssh $H nexus project status flight-efb --json     # status, failure, metrics
ssh $H nexus doctor flight-efb --json             # structured diagnostics
ssh $H nexus logs flight-efb --errors --lines 200 --json
tar -czf - . | ssh $H nexus deploy flight-efb --json --message "agent: fix #42"
ssh $H nexus history flight-efb --json
ssh $H nexus rollback flight-efb --json
ssh $H nexus restart flight-efb --service api-server --json
```
Tip: reuse one SSH connection for many calls (the server rate-limits new
connections to 60/minute per address):
```
# ~/.ssh/config on the agent's machine
Host pixel-agent
    HostName pixel.local
    User agent-claude
    ControlMaster auto
    ControlPath ~/.ssh/cm-%r@%h:%p
    ControlPersist 10m
```

Rules of thumb for agents:
* Always pass `--json`; check the exit code (0 ok, 3 forbidden, 4 not found,
  5 supervisor unreachable, 1 failed).
* On failure read `failure` / `diagnostics[].code`, then `logs`,
  `suggested_actions` and `recovery_actions[].command` (each lists the
  permission it needs).
* NEXUS never edits source code and never runs repairs by itself; fix the
  code locally and redeploy. A failing deploy rolls back automatically, so
  production keeps the last healthy release.

Schema: [json-schema.md](json-schema.md). Manifest: [app-integration.md](app-integration.md).
