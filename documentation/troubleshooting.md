# Troubleshooting

| Symptom | What to check |
|---|---|
| `ssh pixel.local` fails to resolve | mDNS may be blocked on your network or OS (Windows without Bonjour). Use the IP shown by the installer or on the console (`nexus status`). |
| SSH: "REMOTE HOST IDENTIFICATION HAS CHANGED" | Expected only after reinstalling. Otherwise **stop**: compare with the fingerprint from `nexus ssh status` on the console. |
| SSH refuses passwords | After `nexus ssh finalize` only keys work. On the local console: `sudo nexus ssh allow-passwords` (or `nexus ssh allow-passwords` as an admin). |
| Too many connection attempts get dropped | the firewall rate-limits new SSH connections to 12/minute per address; wait a minute. |
| A project shows FAILED / CRASH_LOOP | `nexus doctor <p>` and `nexus logs <p> --errors`; fix, redeploy or `nexus start <p>` to clear the crash-loop stop. |
| Deploy fails with MANIFEST_INVALID | the message names the bad key; see app-integration.md. |
| App unreachable from the LAN | declare the port in `nexus.toml` (`ports = [8080]`, public by default); check `nexus project status <p>` → `listening`. |
| `nexus` says supervisor unreachable | `pidof nexusd`; init restarts it automatically; see `/var/log/messages`. |
| `/data` "degraded" at boot | the data partition could not be checked/mounted; log in on the console, run `sudo e2fsck /dev/disk/…` — see `/run/nexus/fsck.log`. |
| Update did not stick | `sudo nexus system slot`; the trial boot falls back after 3 unconfirmed attempts. Check `/var/log/messages` for `nexus-boot`. |
| Installer: "installation media not found" | boot from the ISO/USB (not a `-kernel` QEMU boot); rewrite the stick if checksums fail. |

Logs: system `/var/log/messages`, install `/var/log/install.log`, project
logs via `nexus logs`, audit `nexus audit`, sudo `/var/log/sudo.log`.
