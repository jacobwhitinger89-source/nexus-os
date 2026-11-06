# Hardware compatibility

| Area | Supported in the alpha | Notes |
|---|---|---|
| CPU | any x86-64 | generic x86-64 build (no -march tuning) |
| Firmware | UEFI (removable-media loader, no NVRAM writes) and legacy BIOS | Secure Boot **not** supported — disable it |
| RAM | ≥ 256 MiB | zram swap enabled automatically below 4 GiB |
| Storage | AHCI/SATA, NVMe, eMMC/SD (SDHCI), USB mass storage/UAS, virtio | software RAID/LVM not supported |
| Ethernet | Intel e1000/e1000e/igb/igc, Realtek r8169 (+ rtl_nic firmware if present), Broadcom tg3, Atheros atl1c/alx, virtio, USB (CDC, AX88179, RTL8152) | others: report hardware with `lspci -nn` |
| Wi-Fi / Bluetooth | **not supported** | servers should use wired networking |
| GPU | detected and listed (PCI); utilisation only where the driver exposes it (amdgpu) | no GPU compute drivers in the base OS |
| Watchdog | Intel TCO (iTCO), AMD SP5100, i6300esb (QEMU); kernel softdog fallback | hardware watchdogs also recover kernel hangs; softdog only userspace hangs |
| Power button | ACPI → clean shutdown | |
| Auto power-on after outage | firmware setting | cannot be set from the OS |

## Tested
| Platform | Result |
|---|---|
| QEMU 11.1 q35, KVM, OVMF (UEFI) | install, boot, deploy, power-loss recovery, A/B update — automated (`make test`) |
| QEMU 11.1 q35, SeaBIOS | boot of installed disk — automated |
| Physical hardware | **not yet tested** — reports welcome |

Please report results (model, `nexus resources --json`, what worked) in a
GitHub issue labelled `hardware`.
