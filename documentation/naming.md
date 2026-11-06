# Project name check

"NEXUS OS" is a **provisional** name. A search (November 2026) found the name
already used by several unrelated projects:

| Existing use | Kind | Risk |
|---|---|---|
| sourceforge.net/projects/nexus-os, /nexusos, /nexusoperatings | hobby / planned operating systems | medium — same category, same name |
| GitLab "Nexus OS" live-boot Linux distribution (p2p + consensus) | Linux distribution | **high** — same category |
| "Nexus OS" clean-slate OS ("Liquid OS Factory") documentation site | operating system | **high** |
| Nexus blockchain "Nexus OS" (nexus.xyz) | product brand | medium |
| Cornell University *Nexus* research OS (trusted computing) | research OS | low (academic, older) |
| Cisco NX-OS, the OS of Cisco Nexus switches | commercial network OS | **high** — trademark owner in the server/network space |
| Sonatype Nexus Repository | developer infrastructure product | medium — strong developer-tool brand |

**Recommendation:** keep "NEXUS" as the internal codename for the alpha but
choose a distinct public name before any 1.0 branding, domain purchase or
package publication. All user-visible branding lives in a few places
(`/etc/nexus-release`, `post-build.sh`, installer banner, dashboard header,
README) so a rename is mechanical. Binary and path names (`nexus`, `nexusd`,
`/data/nexus`) would be renamed in the same change.

This is not legal advice; a trademark search in the target jurisdictions is
needed before public launch.
