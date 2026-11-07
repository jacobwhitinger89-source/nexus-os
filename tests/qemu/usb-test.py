#!/usr/bin/env python3
"""USB stick acceptance test: the ISO written to a USB stick (attached as a
USB mass-storage disk, the way `dd` leaves it) must boot and install under
both UEFI and legacy BIOS firmware, and the installed disk must then boot.

 1. firmware boots the stick (hybrid ISO: GPT/EFI and MBR boot paths)
 2. the installer finds its media on the stick and never offers the stick
    itself as an install target
 3. installation completes; the installed disk boots to a login prompt
"""
import os
import shutil
import subprocess
import sys

sys.path.insert(0, os.path.dirname(__file__))
from harness import WORK, Results, Vm, base_args, ovmf_args  # noqa: E402

iso = sys.argv[1]
PASSWORD = "nexus-test-pw-1"
res = Results("usb-test")


def stick_args(stick):
    return ["-device", "qemu-xhci,id=xhci",
            "-drive", f"file={stick},if=none,id=stick,format=raw",
            "-device", "usb-storage,bus=xhci.0,drive=stick,removable=on,bootindex=0"]


def run(fw):
    disk = os.path.join(WORK, f"usb-{fw}.qcow2")
    stick = os.path.join(WORK, f"usb-{fw}-stick.img")
    vars_path = os.path.join(WORK, f"usb-{fw}-vars.fd")
    for p in (disk, vars_path):
        if os.path.exists(p):
            os.unlink(p)
    shutil.copy(iso, stick)  # a writable copy, like a real stick
    subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", disk, "4G"], check=True)
    firmware = ovmf_args(vars_path) if fw == "uefi" else []
    disk_args = ["-drive", f"file={disk},if=none,id=d0,format=qcow2", "-device", "virtio-blk-pci,drive=d0,bootindex=1"]

    # The stick is chosen first, as in the firmware's one-time boot menu
    # (SeaBIOS, like many BIOSes, only tries the first hard disk in order).
    vm = Vm(base_args(mem=512) + firmware + disk_args + stick_args(stick), f"usb-{fw}-install")
    try:
        vm.expect(r"Welcome to NEXUS", 150)
        res.check(f"{fw}: USB stick boots the installer", True)
        vm.expect(r"Press Enter to begin", 10); vm.sendline()
        vm.expect(r"Hardware check passed", 60)
        vm.expect(r"Press Enter to continue", 60); vm.sendline()
        vm.expect(r"Computer hostname", 10); vm.sendline(f"usb{fw}")
        vm.expect(r"Administrator username", 10); vm.sendline("admin")
        vm.expect(r"Administrator password:\r?\n> ", 10); vm.sendline(PASSWORD)
        vm.expect(r"Confirm password:\r?\n> ", 10); vm.sendline(PASSWORD)
        m = vm.expect(r"(.*?)Select the disk", 60)
        targets = [l for l in m.group(1).splitlines() if l.strip().startswith("[")]
        res.check(f"{fw}: only the internal disk is offered", len(targets) == 1 and "vda" in targets[0],
                  "\n".join(targets))
        vm.sendline("1")
        vm.expect(r"Type the disk name", 10); vm.sendline("vda")
        vm.expect(r"NEXUS OS INSTALLED", 600)
        res.check(f"{fw}: installation from the stick completes", True)
    finally:
        vm.kill()

    # Stick removed: boot the installed disk with the same firmware.
    vm = Vm(base_args(mem=384) + firmware + disk_args, f"usb-{fw}-boot")
    try:
        vm.expect(r"login: ?$", 180)
        res.check(f"{fw}: installed disk boots to login", True)
    finally:
        vm.kill()
    for p in (stick, disk):
        os.unlink(p)


for fw in ("uefi", "bios"):
    try:
        run(fw)
    except Exception as e:  # noqa: BLE001 - record and continue with the other firmware
        res.check(f"{fw}: completed without error", False, repr(e))
sys.exit(res.finish())
