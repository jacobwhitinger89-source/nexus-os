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

## 2. Connect the agent's machine (once)
On the machine the agent runs on, pin the server's host key and write an SSH
alias with connection sharing (see [connect.md](connect.md)); take the
fingerprint from `nexus ssh status` on the server console:
```sh
nexus connect pixel.local --user agent-claude --alias pixel-agent \
      --key ~/.ssh/agent_ed25519.pub --no-enroll \
      --fingerprint SHA256:… --json
```
The alias block contains `ControlMaster auto`, `ControlPath ~/.ssh/nexus-%C`
and `ControlPersist 10m`, so the many short `ssh … nexus … --json` calls an
agent makes reuse one authenticated connection. With the alias, use
`H=pixel-agent` below. (Human administrators run `nexus connect pixel.local`
without `--no-enroll` to enroll their own key with their password.)

## 3. Agent workflow
```sh
H=agent-claude@pixel.local     # or the alias written by nexus connect
ssh $H nexus projects --json
ssh $H nexus project status flight-efb --json     # status, failure, metrics
ssh $H nexus doctor flight-efb --json             # structured diagnostics
ssh $H nexus logs flight-efb --errors --lines 200 --json
tar -czf - . | ssh $H nexus deploy flight-efb --json --message "agent: fix #42"
ssh $H nexus history flight-efb --json
ssh $H nexus rollback flight-efb --json
ssh $H nexus restart flight-efb --service api-server --json
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
