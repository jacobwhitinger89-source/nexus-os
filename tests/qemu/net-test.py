#!/usr/bin/env python3
"""Networking acceptance test with two installed NEXUS machines on one
virtual Ethernet segment (a direct virtual cable; no DHCP server on it):

 1. unattended install (`nexus-install --answers`) of "alpha" and "bravo"
 2. static IPv4 configuration with `nexus system network static`
 3. the machines reach each other; the static setting survives a reboot
 4. mDNS: each machine resolves the other's <hostname>.local
 5. a hostname change is announced over mDNS right away
"""
import os
import random
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from harness import WORK, Results, Vm, base_args, link_net  # noqa: E402

iso = sys.argv[1]
images = os.path.dirname(os.path.abspath(iso))
PASSWORD = "nexus-test-pw-1"
PORT = random.randint(20000, 40000)
res = Results("net-test")
MACHINES = {
    "alpha": {"mac": "52:54:00:4e:58:01", "ip": "10.0.9.1", "port": PORT, "peer": PORT + 1},
    "bravo": {"mac": "52:54:00:4e:58:02", "ip": "10.0.9.2", "port": PORT + 1, "peer": PORT},
}


def disk_path(name):
    return os.path.join(WORK, f"net-{name}.qcow2")


def install(name):
    disk = disk_path(name)
    if os.path.exists(disk):
        os.unlink(disk)
    subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", disk, "4G"], check=True)
    vm = Vm(base_args(mem=512, net=link_net(MACHINES[name]["port"], MACHINES[name]["peer"], MACHINES[name]["mac"])) + [
        "-kernel", os.path.join(images, "bzImage"),
        "-initrd", os.path.join(images, "rootfs.cpio.zst"),
        "-append", "nexus.mode=live nexus.shell=1 console=ttyS0,115200 loglevel=4",
        "-drive", f"file={disk},if=none,id=d0,format=qcow2", "-device", "virtio-blk-pci,drive=d0",
        "-drive", f"file={iso},if=none,id=cd,media=cdrom,readonly=on", "-device", "ide-cd,drive=cd",
    ], f"net-install-{name}")
    try:
        vm.expect(r"[#$] $", 90)
        answers = [f'hostname = "{name}"', 'admin = "admin"', f'password = "{PASSWORD}"',
                   'disk = "vda"', 'confirm_erase = "vda"']
        vm.run("printf '%s\\n' " + " ".join(f"'{l}'" for l in answers) + " > /tmp/answers.toml")
        out, rc = vm.run("nexus-install --answers /tmp/answers.toml >/tmp/install.log 2>&1; echo rc=$?; tail -3 /tmp/install.log",
                         timeout=600)
        return res.check(f"{name}: unattended install", "rc=0" in out, out)
    finally:
        vm.kill()


def boot(name):
    vm = Vm(base_args(mem=384, net=link_net(MACHINES[name]["port"], MACHINES[name]["peer"], MACHINES[name]["mac"])) + [
        "-drive", f"file={disk_path(name)},if=none,id=d0,format=qcow2", "-device", "virtio-blk-pci,drive=d0,bootindex=0",
    ], f"net-{name}")
    login(vm)
    return vm


def login(vm):
    vm.expect(r"login: ?$", 180)
    vm.sendline("admin")
    vm.expect(r"Password: ?$", 20)
    vm.sendline(PASSWORD)
    vm.expect(r"[#$] $", 30)


def sudo(vm, cmd, timeout=60):
    return vm.run(f"echo '{PASSWORD}' | sudo -S -p '' {cmd}", timeout=timeout)


def mdns_browse(vm, host, ip, tries=6):
    """True when `host`'s SSH service is announced over multicast DNS under
    exactly that name (a "-2" suffix would mean a false name conflict)."""
    # -i: mquery otherwise picks the default-route interface, and this
    # segment has no gateway.
    want = f"+ {host}._ssh._tcp.local. ({ip})"
    for _ in range(tries):
        out, _ = vm.run("mquery -i $(cat /run/nexus/net-interface) -w 4 2>&1", timeout=30)
        if want in out.splitlines():
            return True, out
        time.sleep(2)
    return False, out


def mdns_resolve(vm, host, ip):
    """Ask the responder at `ip` for `host`.local's A record (unicast to
    port 5353; mquery's one-shot A queries can hang)."""
    out, rc = vm.run(f"nslookup -type=a -port=5353 -timeout=3 {host}.local {ip} 2>&1", timeout=30)
    return rc == 0 and f"Address: {ip}" in out, out


vms = {}
try:
    if not all([install("alpha"), install("bravo")]):
        res.finish()

    for name in MACHINES:
        vms[name] = boot(name)
    res.check("both installed machines boot to login", True)

    for name, m in MACHINES.items():
        out, rc = sudo(vms[name], f"nexus system network static --address {m['ip']}/24 --json")
        res.check(f"{name}: static address accepted", rc == 0 and '"network": "static"' in out, out)
    for name, m in MACHINES.items():
        ok = False
        for _ in range(15):
            out, _ = vms[name].run("ip -4 -o addr show")
            if f"{m['ip']}/24" in out:
                ok = True
                break
            time.sleep(1)
        res.check(f"{name}: static address applied", ok, out)

    out, rc = vms["alpha"].run("ping -c 3 -W 2 10.0.9.2")
    res.check("alpha reaches bravo over the static network", rc == 0, out)

    # Static configuration lives on /data and must survive a reboot.
    vms["bravo"].sendline(f"echo '{PASSWORD}' | sudo -S -p '' nexus system reboot")
    login(vms["bravo"])
    out, _ = vms["bravo"].run("sleep 3; ip -4 -o addr show")
    res.check("bravo: static address persists across reboot", "10.0.9.2/24" in out, out)
    out, rc = vms["bravo"].run("ping -c 3 -W 2 10.0.9.1")
    res.check("bravo reaches alpha after reboot", rc == 0, out)

    ok, out = mdns_browse(vms["alpha"], "bravo", "10.0.9.2")
    res.check("mDNS: alpha discovers bravo under its own name", ok, out)
    # alpha got its address while running (no reboot), so mdnsd must have
    # waited for it / been restarted cleanly.
    ok, out = mdns_browse(vms["bravo"], "alpha", "10.0.9.1")
    res.check("mDNS: bravo discovers alpha under its own name", ok, out)
    ok, out = mdns_resolve(vms["alpha"], "bravo", "10.0.9.2")
    res.check("mDNS: bravo.local resolves to its address", ok, out)
    ok, out = mdns_resolve(vms["bravo"], "alpha", "10.0.9.1")
    res.check("mDNS: alpha.local resolves to its address", ok, out)

    out, rc = sudo(vms["alpha"], "nexus system hostname charlie --json")
    res.check("alpha renamed to charlie", rc == 0 and '"charlie"' in out, out)
    ok, out = mdns_browse(vms["bravo"], "charlie", "10.0.9.1")
    res.check("mDNS: new hostname announced", ok, out)
    ok, out = mdns_resolve(vms["bravo"], "charlie", "10.0.9.1")
    res.check("mDNS: charlie.local resolves after the rename", ok, out)
finally:
    for vm in vms.values():
        vm.kill()

res.finish()
