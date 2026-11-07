# Hardware compatibility

| Area | Supported in the alpha | Notes |
|---|---|---|
| CPU | any x86-64 | generic x86-64 build (no -march tuning) |
| Firmware | UEFI (removable-media loader, no NVRAM writes) and legacy BIOS | Secure Boot **not** supported — disable it |
| RAM | ≥ 256 MiB | zram swap enabled automatically below 4 GiB |
| Storage | AHCI/SATA, NVMe, eMMC/SD (SDHCI), USB mass storage/UAS, virtio; server RAID/HBA: Broadcom/LSI MegaRAID and SAS3 (mpt3sas), HPE Smart Array (hpsa), Microchip SmartPQI, VMware PVSCSI | software RAID/LVM not supported; with a hardware RAID card, create the volume in the card's setup utility first — the installer sees it as one disk |
| Ethernet | Intel e1000/e1000e/igb/igc and 10/25/40/100G (ixgbe, i40e, ice), Realtek r8169 incl. 2.5G RTL8125 (rtl_nic firmware included), Broadcom tg3 and NetXtreme-C/E (bnxt), Mellanox ConnectX-4+ (mlx5), Aquantia/Marvell AQtion, Atheros atl1c/alx, virtio, vmxnet3, USB (CDC, AX88179, RTL8152) | Intel E810 (ice) runs in its basic "safe mode" (no DDP package) | others: report hardware with `lspci -nn` |
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
| ISO written to a USB stick (QEMU xHCI usb-storage), UEFI and BIOS | boots, installs, installed disk boots — automated (`make test-usb`) |
| Physical hardware | **not yet tested** — reports welcome |

Please report results (model, `nexus resources --json`, what worked) in a
GitHub issue labelled `hardware`.
