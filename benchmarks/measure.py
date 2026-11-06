#!/usr/bin/env python3
"""Boot the live image several times and measure boot time, memory and
daemon footprint. Results are real measurements from this host's QEMU."""
import json
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "tests", "qemu"))
from harness import Vm, base_args  # noqa: E402

img, out_path = sys.argv[1], sys.argv[2]
sizes = {f: os.path.getsize(os.path.join(img, f)) for f in os.listdir(img)
         if f.endswith((".iso", ".squashfs", ".zst", ".tar")) or f == "bzImage"}
runs = []
for i in range(3):
    vm = Vm(base_args(mem=256) + ["-kernel", os.path.join(img, "bzImage"),
                                  "-initrd", os.path.join(img, "rootfs.cpio.zst"),
                                  "-append", "nexus.mode=live nexus.shell=1 console=ttyS0,115200 loglevel=1"],
            f"bench-{i}")
    try:
        t0 = time.time()
        vm.expect(r"[#$] $", 120)
        boot = time.time() - t0
        time.sleep(10)  # settle: idle measurement
        meminfo, _ = vm.run("cat /proc/meminfo")
        m = {l.split(":")[0]: int(l.split()[1]) for l in meminfo.strip().splitlines() if l.split()[1:2]}
        rss_out, _ = vm.run("for p in /proc/[0-9]*; do n=$(cat $p/comm); r=$(awk '/VmRSS/{print $2}' $p/status); [ -n \"$r\" ] && echo \"$n $r\"; done")
        rss = {}
        for line in rss_out.strip().splitlines():
            parts = line.split()
            if len(parts) == 2 and parts[1].isdigit():
                rss[parts[0]] = rss.get(parts[0], 0) + int(parts[1])
        load, _ = vm.run("cat /proc/loadavg")
        runs.append({"boot_to_shell_s": round(boot, 2),
                     "mem_total_kib": m["MemTotal"],
                     "mem_used_kib": m["MemTotal"] - m["MemAvailable"],
                     "rss_kib": rss, "loadavg": load.split()[:3]})
        vm.sendline("poweroff")
        vm.wait_exit(60)
    finally:
        vm.kill()

result = {
    "note": "live mode: the whole root filesystem is in RAM (initramfs), so used memory includes it; installed systems run from squashfs on disk",
    "image_sizes_bytes": sizes,
    "boot_to_shell_s_median": statistics.median(r["boot_to_shell_s"] for r in runs),
    "idle_mem_used_kib_median": statistics.median(r["mem_used_kib"] for r in runs),
    "runs": runs,
}
with open(out_path, "w") as f:
    json.dump(result, f, indent=2)
print(json.dumps({k: v for k, v in result.items() if k != "runs"}, indent=2))
print("daemon RSS (KiB):", json.dumps(runs[-1]["rss_kib"]))
