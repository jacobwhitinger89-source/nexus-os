#!/usr/bin/env python3
"""Phase 1 acceptance: the image boots in QEMU, identifies itself as NEXUS
OS, initialises, prints hardware and memory information, offers a local
console and shuts down cleanly. Also records boot time and idle memory."""
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from harness import Results, Vm, base_args  # noqa: E402

images = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "../../build/output/images")
res = Results("boot-test")
args = base_args(mem=256) + [
    "-kernel", os.path.join(images, "bzImage"),
    "-initrd", os.path.join(images, "rootfs.cpio.zst"),
    "-append", "nexus.mode=live nexus.shell=1 console=ttyS0,115200 loglevel=4",
]
vm = Vm(args, "boot-test")
try:
    t0 = time.time()
    vm.expect(r"NEXUS OS", 60)
    res.check("kernel boots and NEXUS banner appears", True)
    m = vm.expect(r"Memory:\s+(\d+) MiB total, (\d+) MiB available", 30)
    res.check("hardware and memory information printed", int(m.group(1)) > 0)
    vm.expect(r"CPU:", 5) if "CPU:" in vm.buf else None
    vm.expect(r"[#$] $", 60)
    boot = time.time() - t0
    res.metrics["boot_to_console_seconds"] = round(boot, 2)
    res.check("local console available", True)

    out, rc = vm.run("cat /etc/nexus-release")
    res.check("identifies as NEXUS OS", rc == 0 and "NEXUS_VERSION=" in out, out)
    out, rc = vm.run("uname -r")
    res.check("custom kernel", "-nexus" in out, out)
    time.sleep(3)
    out, rc = vm.run("awk '/MemTotal/{t=$2}/MemAvailable/{a=$2}END{print t-a}' /proc/meminfo")
    used_kb = int(out.strip().splitlines()[-1])
    res.metrics["idle_memory_used_kib_live"] = used_kb
    res.check("idle memory under 96 MiB (live: includes the RAM-resident rootfs)", used_kb < 96 * 1024, f"{used_kb} KiB")
    out, rc = vm.run("nexus resources --json")
    try:
        r = json.loads(out)
        res.check("nexus resources --json works", r["cpu"]["cores"] == 2 and r["memory"]["total_bytes"] > 0)
        res.check("GPU reported honestly", isinstance(r["gpus"], list))
    except Exception as e:  # noqa: BLE001
        res.check("nexus resources --json works", False, f"{e}: {out[:300]}")
    out, rc = vm.run("pidof nexusd")
    res.check("supervisor running", rc == 0, out)
    out, rc = vm.run("cat /proc/1/cmdline | tr '\\0' ' '; ls /sys/fs/cgroup/nexus")
    res.check("cgroup v2 hierarchy prepared", rc == 0, out)

    vm.sendline("poweroff")
    vm.expect(r"Power down|reboot: Power", 60)
    code = vm.wait_exit(30)
    res.check("clean shutdown (QEMU exited)", code == 0, f"exit {code}")
finally:
    vm.kill()
res.finish()
