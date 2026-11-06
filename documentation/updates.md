# A/B system updates

The system lives in two read-only squashfs slots (NEXUS_A, NEXUS_B). Kernels
live on the EFI system partition at `/nexus/<slot>/bzImage`. GRUB keeps the
boot state in `/grub/grubenv` on the ESP:

| variable | meaning |
|---|---|
| `nexus_slot` | known-good slot (`a`/`b`) |
| `nexus_try` | slot to try ("" when none) |
| `nexus_tries` | remaining attempts; GRUB decrements before each trial boot |
| `nexus_partuuid_a/_b` | root partitions |

Update flow:
1. `sudo nexus system update nexus-<ver>-x86_64-update.tar` verifies the
   bundle's SHA256SUMS, writes the image to the **inactive** slot, verifies
   it by reading it back, installs the kernel and sets `nexus_try=<slot>`,
   `nexus_tries=3`. The running system is untouched.
2. Reboot. GRUB boots the trial slot with `nexus.trial=1`.
3. `/usr/libexec/nexus/confirm-boot` waits 90 s; if nexusd and the SSH
   server are running it calls `nexus system confirm-boot`, making the new
   slot `nexus_slot`.
4. If the new system panics (auto-reboot after 10 s), hangs (hardware
   watchdog, where present) or never confirms, each reboot uses one attempt;
   after three, GRUB clears `nexus_try` and boots the previous slot.

`sudo nexus system slot` shows the state. The data partition is shared by
both slots, so configuration and projects survive updates and rollbacks.

Limitations: bundles are checksummed but not signed; a hang without a
hardware watchdog needs a manual power cycle to reach the next attempt.
