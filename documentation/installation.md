# Installing NEXUS OS

**Alpha software.** Installation erases the selected disk completely.

## Requirements
* x86-64 computer (any 64-bit Intel/AMD CPU), UEFI or legacy BIOS
* 256 MiB RAM minimum, 512 MiB+ recommended
* a disk of at least 4 GiB (SATA, NVMe, eMMC, USB, virtio)
* wired Ethernet (Wi-Fi is not supported in the alpha — see hardware.md)

## 1. Write the USB stick
Download `nexus-<version>-x86_64.iso` and `SHA256SUMS` from the GitHub
release and verify:
```sh
sha256sum -c SHA256SUMS --ignore-missing
```
* Linux: `sudo dd if=nexus-<version>-x86_64.iso of=/dev/sdX bs=4M conv=fsync status=progress`
  (double-check `/dev/sdX` with `lsblk` — this overwrites it)
* macOS: `diskutil list`, `diskutil unmountDisk /dev/diskN`, then
  `sudo dd if=nexus.iso of=/dev/rdiskN bs=4m`
* Windows: use Rufus (DD mode) or balenaEtcher.

## 2. Install
Boot from the stick (firmware boot menu: often F12, F11, F8 or Esc). Choose
**Install NEXUS OS**. The wizard asks for:

1. welcome → 2. hardware check → 3. network detection → 4. hostname →
5. administrator username → 6–7. password (hidden, entered twice) →
8. disk (the USB stick itself is never offered) → 9. type the disk's name
(e.g. `sda`) to confirm erasing it → 10. installation → 11. reboot.

The final screen shows the IP address, `ssh admin@<hostname>.local` and the
SSH host-key fingerprints. **Write the fingerprint down** (or photograph
it).

Enable "Restore on AC power loss" / "Power on after power failure" in the
firmware setup so the server restarts by itself after a power cut. NEXUS
cannot change this setting from software.

## 3. First connection and key setup
On your computer:

* **Linux / macOS**
  ```sh
  ssh admin@pixel.local          # compare the fingerprint shown with the installer's, type yes
  ssh-keygen -t ed25519          # only if you have no key yet (~/.ssh/id_ed25519.pub)
  ssh-copy-id admin@pixel.local  # enter the password once more
  ssh -o PasswordAuthentication=no admin@pixel.local nexus ssh finalize
  ```
* **Windows 10/11** (PowerShell, built-in OpenSSH)
  ```powershell
  ssh admin@pixel.local
  ssh-keygen -t ed25519
  type $env:USERPROFILE\.ssh\id_ed25519.pub | ssh admin@pixel.local "nexus ssh add-key \"$(cat)\""
  ssh -o PasswordAuthentication=no admin@pixel.local nexus ssh finalize
  ```
  If `pixel.local` does not resolve (no mDNS/Bonjour), use the IP address
  from the installer.

`nexus ssh finalize` disables password logins over SSH (it refuses until
you have logged in with a key, so you cannot lock yourself out). The local
console keeps password login for recovery.

Optional: add an alias to `~/.ssh/config` so `ssh pixel` works:
```
Host pixel
    HostName pixel.local
    User admin
```

## 4. Next steps
* `nexus` (or the dashboard shown at login)
* create and deploy a project: [app-integration.md](app-integration.md)
* give an AI agent access: [agents.md](agents.md)
