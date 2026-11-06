# `nexus connect` — set up SSH access from your computer

`nexus connect` runs on **your** Linux or macOS machine (or an agent's), not
on the NEXUS server. It replaces the manual `ssh-copy-id` + `known_hosts` +
`~/.ssh/config` steps with one verified command.

```sh
nexus connect pixel.local                 # interactive
nexus connect 192.168.1.20 --alias pixel --user admin \
      --fingerprint SHA256:MYpnLW8MrEoCUbqpj/MNzwY/D0igh0J4YLv8g1mFlSg --json
```

## Getting it
* As part of the `nexus` CLI: `cargo install --path management/cli`
  (`nexus connect …`).
* Standalone, small, no Linux-only code (macOS build not yet tested in CI):
  `cargo install --path management/connect` (`nexus-connect …`, same options).

Needs the OpenSSH client (`ssh`, `ssh-keyscan`) on your computer.

## What it does
1. Fetches the server's ed25519 host key with `ssh-keyscan` and computes its
   SHA256 fingerprint (same format as `ssh-keygen -l`).
2. Asks you to compare it with the fingerprint shown on the **server
   console** by `nexus ssh status` (type `yes` or the last 6 characters), or
   checks it against `--fingerprint` without asking. A mismatch stops
   everything (`HOST_KEY_MISMATCH`, exit 1) before any file is changed or any
   password is typed.
3. Adds the key to `~/.ssh/known_hosts` (`[host]:port` form for non-22
   ports) unless it is already there. Older entries for the same host are
   left alone and reported (`stale_entries`, remove with `ssh-keygen -R`).
4. Enrolls your **public** key (default `~/.ssh/id_ed25519.pub`, or `--key`)
   by running `nexus ssh add-key '<key line>'` over a normal SSH login with
   strict host key checking against the key just pinned. You type the
   account password once if no key works yet. The private key is never read
   or transmitted; afterwards a password-less test login
   (`nexus whoami`) sets `key_login_verified`.
5. Writes a marked block to `~/.ssh/config`:
   ```
   # >>> nexus connect: pixel >>>
   # Managed by nexus connect; re-run it to update, delete the block to remove.
   Host pixel
       HostName pixel.local
       User admin
       IdentityFile ~/.ssh/id_ed25519
       StrictHostKeyChecking yes
       ControlMaster auto
       ControlPath ~/.ssh/nexus-%C
       ControlPersist 10m
       ServerAliveInterval 30
   # <<< nexus connect: pixel <<<
   ```
   Re-running replaces only this block; everything else in the file is kept
   byte for byte. A new block is appended. If you already have your own
   `Host pixel` entry it is not touched and `alias_conflict` is reported
   (an earlier entry wins in OpenSSH).

Then: `ssh pixel nexus status`, and once key login works,
`ssh pixel nexus ssh finalize` to disable password logins.

## Options
| Option | Default | |
|---|---|---|
| `--user U` | `admin` | account on the server |
| `--port N` | `22` | SSH port |
| `--alias A` | host without `.local` | `Host` name in `~/.ssh/config` |
| `--key FILE.pub` | `~/.ssh/id_ed25519.pub` | public key to enroll |
| `--fingerprint SHA256:…` | ask | expected host key (required without a terminal) |
| `--no-enroll` | | only pin the host key (and write the alias) |
| `--no-config` | | do not touch `~/.ssh/config` |
| `--ssh-dir DIR` | `~/.ssh` | alternate directory (testing) |
| `--json` | | result as a `kind: "connect"` envelope |

Exit codes match the rest of the CLI: 0 ok, 1 failed (`HOST_KEY_MISMATCH`,
`NOT_CONFIRMED`, `ENROLL_FAILED`, `CONFIG_DAMAGED`, `MISSING_TOOL`), 2 usage
(also: no terminal and no `--fingerprint`), 4 public key file not found,
5 server unreachable / no ed25519 host key.

## JSON result
```json
{ "schema_version": 1, "kind": "connect", "generated_at": "…",
  "host": "pixel.local", "port": 22, "user": "admin", "alias": "pixel",
  "host_key": { "type": "ssh-ed25519", "fingerprint": "SHA256:…", "confirmed_by": "flag" },
  "known_hosts": { "file": "…", "pattern": "pixel.local", "added": true, "stale_entries": 0 },
  "key": { "file": "…/id_ed25519.pub", "enrolled": true, "key_login_verified": true },
  "ssh_config": { "file": "…/config", "written": true, "alias_conflict": false },
  "next": "ssh pixel nexus status" }
```
`confirmed_by` is `flag` or `interactive`; `key_login_verified` is `null`
when no matching private key file exists next to the `.pub` file.

## For agent accounts
Agent keys are created by an administrator (`nexus agent add`), not with
`nexus connect`'s password step. On the agent's machine, pin the host key and
write the alias without enrolling:
```sh
nexus connect pixel.local --user agent-claude --alias pixel-agent \
      --key ~/.ssh/agent_ed25519.pub --no-enroll --fingerprint SHA256:… --json
```
(`--key` also sets `IdentityFile` when the private key sits next to it.)
`nexus connect` is refused inside the agent forced command.
