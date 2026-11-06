#!/usr/bin/env python3
"""End-to-end acceptance test, entirely inside QEMU (no host disks touched).

 1. boot the installer ISO (UEFI) with a blank virtual disk
 2. drive the interactive installer over the serial console
 3. boot the installed system; verify the SSH host key fingerprint
 4. password bootstrap -> key enrollment -> `nexus ssh finalize`
 5. create a project, deploy the hello-http example, reach it through the
    firewall, read application metrics, crash it and watch it restart
 6. agent account: restricted forced command and project-scoped permissions
 7. process isolation of the service (uid, seccomp, no_new_privs, cgroup)
 8. simulated power loss (hard reset) -> automatic recovery
 9. A/B system update -> trial boot -> automatic confirmation
10. ACPI power button -> clean shutdown
11. the same disk boots with legacy BIOS (SeaBIOS)
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request

sys.path.insert(0, os.path.dirname(__file__))
from harness import TOP, WORK, Results, Vm, base_args, ovmf_args  # noqa: E402

iso = sys.argv[1]
images = os.path.dirname(os.path.abspath(iso))
version = open(os.path.join(TOP, "VERSION")).read().strip()
bundle = os.path.join(images, f"nexus-{version}-x86_64-update.tar")
SSH_PORT, APP_PORT = 2322, 18080
PASSWORD = "nexus-test-pw-1"
disk = os.path.join(WORK, "e2e-disk.qcow2")
vars_path = os.path.join(WORK, "e2e-OVMF_VARS.fd")
for p in (disk, vars_path):
    if os.path.exists(p):
        os.unlink(p)
subprocess.run(["qemu-img", "create", "-q", "-f", "qcow2", disk, "8G"], check=True)
tmp = tempfile.mkdtemp(prefix="nexus-e2e-")
res = Results("install-test")


def keygen(name):
    path = os.path.join(tmp, name)
    subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", name, "-f", path], check=True)
    return path, open(path + ".pub").read().strip()


admin_key, admin_pub = keygen("admin_key")
agent_key, agent_pub = keygen("agent_key")
known_hosts = os.path.join(tmp, "known_hosts")
askpass = os.path.join(tmp, "askpass.sh")
with open(askpass, "w") as f:
    f.write(f"#!/bin/sh\necho '{PASSWORD}'\n")
os.chmod(askpass, 0o700)


def ssh(cmd, user="admin", key=None, password=False, stdin=None, timeout=120):
    opts = ["-p", str(SSH_PORT), "-o", f"UserKnownHostsFile={known_hosts}",
            "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=10",
            "-o", "LogLevel=ERROR", "-o", "IdentitiesOnly=yes"]
    env = dict(os.environ)
    if password:
        opts += ["-o", "PubkeyAuthentication=no", "-o", "PreferredAuthentications=password,keyboard-interactive",
                 "-o", "NumberOfPasswordPrompts=1"]
        env.update(SSH_ASKPASS=askpass, SSH_ASKPASS_REQUIRE="force", DISPLAY=":0")
    else:
        opts += ["-o", "PasswordAuthentication=no", "-o", "KbdInteractiveAuthentication=no", "-i", key or admin_key]
    p = subprocess.run(["ssh", *opts, f"{user}@127.0.0.1", cmd], input=stdin, capture_output=True,
                       timeout=timeout, env=env, stdin=None if stdin is not None else subprocess.DEVNULL)
    return p.returncode, p.stdout.decode(errors="replace"), p.stderr.decode(errors="replace")


def ssh_json(cmd, **kw):
    rc, out, err = ssh(cmd, **kw)
    try:
        return rc, json.loads(out)
    except json.JSONDecodeError:
        return rc, {"_raw": out, "_err": err}


def wait_ssh(timeout=120):
    deadline = time.time() + timeout
    while time.time() < deadline:
        p = subprocess.run(["ssh-keyscan", "-p", str(SSH_PORT), "-t", "ed25519", "127.0.0.1"],
                           capture_output=True, timeout=15)
        if p.returncode == 0 and p.stdout.strip():
            return p.stdout.decode()
        time.sleep(2)
    raise TimeoutError("ssh did not come up")


def http(path, timeout=5):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{APP_PORT}{path}", timeout=timeout) as r:
            return r.status, r.read().decode()
    except Exception as e:  # noqa: BLE001
        return None, str(e)


def boot_disk(name, uefi=True):
    args = base_args(mem=512, ssh_port=SSH_PORT, app_port=APP_PORT)
    if uefi:
        args += ovmf_args(vars_path)
    args += ["-drive", f"file={disk},if=none,id=d0,format=qcow2", "-device", "virtio-blk-pci,drive=d0,bootindex=0"]
    return Vm(args, name)


# ---------------------------------------------------------------- install
vm = Vm(base_args(mem=512, ssh_port=SSH_PORT, app_port=APP_PORT) + ovmf_args(vars_path) + [
    "-drive", f"file={disk},if=none,id=d0,format=qcow2", "-device", "virtio-blk-pci,drive=d0,bootindex=0",
    "-drive", f"file={iso},if=none,id=cd,media=cdrom,readonly=on", "-device", "ide-cd,drive=cd,bootindex=1",
], "e2e-install")
try:
    vm.expect(r"Welcome to NEXUS", 120)
    res.check("installer ISO boots (UEFI)", True)
    vm.expect(r"Press Enter to begin", 10)
    vm.sendline()
    vm.expect(r"Hardware check passed", 60)
    res.check("hardware check passes", True)
    m = vm.expect(r"(Connected: \S+ \S+|No network address yet)", 60)
    res.check("network detected", m.group(1).startswith("Connected"), m.group(1))
    vm.expect(r"Press Enter to continue", 10)
    vm.sendline()
    vm.expect(r"Computer hostname", 10); vm.sendline("Bad_Host")
    vm.expect(r"Use 1-63 characters", 10)
    res.check("invalid hostname rejected", True)
    vm.expect(r"Computer hostname", 10); vm.sendline("pixel")
    vm.expect(r"Administrator username", 10); vm.sendline("admin")
    vm.expect(r"Administrator password:\r?\n> ", 10); vm.sendline(PASSWORD)
    vm.expect(r"Confirm password:\r?\n> ", 10); vm.sendline("mismatch-123")
    vm.expect(r"do not match", 10)
    res.check("password mismatch detected", True)
    vm.expect(r"Administrator password:\r?\n> ", 10); vm.sendline(PASSWORD)
    vm.expect(r"Confirm password:\r?\n> ", 10); vm.sendline(PASSWORD)
    m = vm.expect(r"\[1\] (\w+)\s", 60)
    target = m.group(1)
    res.check("ISO media excluded from disk list", target == "vda", target)
    vm.expect(r"Select the disk", 10); vm.sendline("1")
    vm.expect(r"Type the disk name", 10); vm.sendline("wrong")
    vm.expect(r"Confirmation did not match", 10)
    res.check("wrong erase confirmation refused", True)
    vm.expect(r"Select the disk", 20); vm.sendline("1")
    vm.expect(r"Type the disk name", 10); vm.sendline(target)
    t_install = time.time()
    vm.expect(r"NEXUS OS INSTALLED", 600)
    res.metrics["install_seconds"] = round(time.time() - t_install, 1)
    res.check("installation completes", True)
    m = vm.expect(r"ed25519\s+(SHA256:\S+)", 30)
    fingerprint = m.group(1)
    res.check("password never echoed", PASSWORD not in open(os.path.join(WORK, "e2e-install.log"), errors="replace").read())
    vm.expect(r"press Enter to reboot", 30)
    vm.kill()  # equivalent to removing the media and rebooting
finally:
    vm.kill()

# ---------------------------------------------------------------- first boot
vm = boot_disk("e2e-boot1")
try:
    t0 = time.time()
    vm.expect(r"NEXUS OS \(slot a\)|Booting|Linux version", 120)
    vm.expect(r"login:", 120)
    res.metrics["installed_boot_to_login_seconds"] = round(time.time() - t0, 1)
    res.check("installed system boots to login (UEFI)", True)

    scanned = wait_ssh()
    with open(known_hosts, "w") as f:
        f.write(scanned)
    p = subprocess.run(["ssh-keygen", "-lf", known_hosts], capture_output=True, text=True)
    res.check("SSH host key matches installer fingerprint", fingerprint in p.stdout, f"{fingerprint} vs {p.stdout}")

    rc, st = ssh_json("nexus status --json", password=True)
    res.check("password bootstrap login works", rc == 0 and st.get("hostname") == "pixel", str(st)[:300])
    res.check("password logins flagged as bootstrap", st.get("ssh", {}).get("password_auth") is True)
    res.check("supervisor reports full isolation", st.get("supervisor", {}).get("isolation") == "full", str(st.get("supervisor")))

    rc, out, err = ssh(f"nexus ssh finalize --json", password=True)
    res.check("finalize refused before any key login", rc != 0 and "NOT_VERIFIED" in out, out + err)
    rc, out, err = ssh(f"nexus ssh add-key '{admin_pub}'", password=True)
    res.check("public key enrolled", rc == 0, out + err)
    rc, out, err = ssh("nexus ssh finalize --json")
    res.check("key login + finalize", rc == 0 and '"password_auth": false' in out, out + err)
    time.sleep(2)
    wait_ssh(30)
    rc, out, err = ssh("true", password=True)
    res.check("password logins now rejected", rc != 0, out + err)

    # ------------------------------------------------------------ deploy
    rc, out, err = ssh("nexus project create hello --memory 128 --json")
    res.check("project created", rc == 0, out + err)
    dist = os.path.join(tmp, "dist")
    os.makedirs(dist)
    cargo = shutil.which("cargo") or os.path.expanduser("~/.cargo/bin/cargo")
    subprocess.run([cargo, "build", "-q", "--release", "--target", "x86_64-unknown-linux-musl", "-p", "hello-http"],
                   cwd=TOP, check=True)
    shutil.copy(os.path.join(TOP, "target/x86_64-unknown-linux-musl/release/hello-http"), dist)
    shutil.copy(os.path.join(TOP, "examples/hello-http/nexus.toml"), dist)
    tarball = subprocess.run(["tar", "-czf", "-", "-C", dist, "."], capture_output=True, check=True).stdout
    rc, out, err = ssh("nexus deploy hello --json", stdin=tarball, timeout=240)
    try:
        dep = json.loads(out)
    except json.JSONDecodeError:
        dep = {}
    res.check("deploy succeeds", rc == 0 and dep.get("deployment", {}).get("status") == "succeeded", out[-800:] + err[-500:])
    code, body = http("/")
    res.check("app reachable through firewall", code == 200 and "Hello from NEXUS" in body, f"{code} {body}")
    http("/health"); http("/nope")
    time.sleep(1)
    rc, st = ssh_json("nexus project status hello --json")
    metrics = (st.get("metrics") or {}).get("web", {}).get("metrics", {})
    res.check("application metrics published", metrics.get("http_requests", 0) >= 2, str(st.get("metrics")))
    res.check("project memory accounted", (st.get("resources") or {}).get("memory_bytes") is not None)
    pid = st["services"][0]["pid"] if st.get("services") else None

    rc, out, _ = ssh(f"cat /proc/{pid}/status; cat /proc/{pid}/cgroup")
    res.check("service runs as project user", "Uid:\t2000\t2000\t2000\t2000" in out, out[:400])
    res.check("service has seccomp filter", "Seccomp:\t2" in out)
    res.check("service has no_new_privs", "NoNewPrivs:\t1" in out)
    res.check("service in its cgroup", "0::/nexus/hello/web" in out)
    rc, out, _ = ssh("ls -l /dev/null /dev/zero /dev/urandom")
    res.check("device permissions correct (/dev/null 0666)", out.count("crw-rw-rw-") >= 2, out)

    http("/crash")
    time.sleep(5)
    rc, st = ssh_json("nexus project status hello --json")
    svc = st["services"][0]
    res.check("crashed service restarted", svc["restarts"] >= 1 and (svc.get("last_exit") or {}).get("code") == 3, str(svc))
    rc, doc = ssh_json("nexus doctor hello --json")
    codes = [d["code"] for d in doc.get("diagnostics", [])]
    res.check("crash diagnosed (EXITED_NONZERO) or recovered", rc == 0, str(codes))
    rc, logs = ssh_json("nexus logs hello --errors --json")
    res.check("stderr captured in error log", any("crash requested" in l for l in logs.get("lines", [])), str(logs)[:300])

    # ------------------------------------------------------------ agent
    rc, out, err = ssh(f"nexus agent add claude --key '{agent_pub}' --json")
    res.check("agent account created", rc == 0, out + err)
    rc, out, err = ssh("nexus access grant agent-claude hello read,logs,restart --json")
    res.check("agent granted read,logs,restart", rc == 0, out + err)
    rc, st = ssh_json("nexus project status hello --json", user="agent-claude", key=agent_key)
    res.check("agent reads project status", rc == 0 and st.get("project") == "hello", str(st)[:200])
    rc, out, err = ssh("nexus deploy hello", user="agent-claude", key=agent_key, stdin=tarball)
    res.check("agent cannot deploy without permission", rc == 3, f"rc={rc} {out}{err}")
    rc, out, err = ssh("cat /etc/passwd", user="agent-claude", key=agent_key)
    res.check("agent key limited to nexus commands", rc != 0 and "root:" not in out, out + err)
    rc, out, err = ssh("nexus restart hello --json", user="agent-claude", key=agent_key)
    res.check("agent may restart granted project", rc == 0, out + err)
    rc, aud = ssh_json("nexus audit --json")
    actions = [(e["user"], e["action"], e["result"]) for e in aud.get("entries", [])]
    res.check("denied agent deploy audited", ("agent-claude", "deploy", "denied") in actions, str(actions[-6:]))

    rc, out, _ = ssh("nexus resources --json")
    r = json.loads(out) if rc == 0 else {}
    res.check("resources: storage and network present", bool(r.get("storage")) and bool(r.get("network")))
    rc, out, _ = ssh("awk '/MemTotal/{t=$2}/MemAvailable/{a=$2}END{print t-a}' /proc/meminfo")
    res.metrics["installed_memory_used_kib_with_app_and_ssh"] = int(out.strip() or 0)

    # ------------------------------------------------------------ power loss
    for _ in range(20):
        if http("/")[0] == 200:
            break
        time.sleep(1)
    vm.monitor("system_reset")
    vm.expect(r"login:", 180)
    wait_ssh()
    ok = False
    for _ in range(30):
        code, body = http("/")
        if code == 200:
            ok = True
            break
        time.sleep(2)
    res.check("power loss: system and project recover automatically", ok, body)
    rc, out, _ = ssh("cat /run/nexus/data-state /run/nexus/fsck.rc")
    res.check("power loss: data partition mounted after check", out.split()[:1] == ["ok"], out)

    # ------------------------------------------------------------ A/B update
    if os.path.exists(bundle):
        p = subprocess.run(["scp", "-O", "-P", str(SSH_PORT), "-i", admin_key, "-o", f"UserKnownHostsFile={known_hosts}",
                            "-o", "LogLevel=ERROR", bundle, "admin@127.0.0.1:/data/home/admin/update.tar"],
                           capture_output=True, text=True)
        res.check("scp upload works", p.returncode == 0, p.stderr)
        rc, out, err = ssh(f"echo '{PASSWORD}' | sudo -S -p '' nexus system update /data/home/admin/update.tar --json",
                           timeout=300)
        res.check("update written to slot b", rc == 0 and '"slot": "b"' in out, out + err)
        ssh(f"echo '{PASSWORD}' | sudo -S -p '' nexus system reboot", timeout=30)
        vm.expect(r"nexus\.slot=b|slot b", 120)
        vm.expect(r"login:", 180)
        wait_ssh()
        time.sleep(100)  # confirm-boot waits 90 s of healthy operation
        rc, out, err = ssh(f"echo '{PASSWORD}' | sudo -S -p '' nexus system slot --json")
        try:
            slot = json.loads(out)
        except json.JSONDecodeError:
            slot = {}
        res.check("trial boot of slot b confirmed", slot.get("booted_slot") == "b" and slot.get("good_slot") == "b"
                  and slot.get("pending_slot") == "", out + err)
        res.check("project survives system update", http("/")[0] == 200)

        # A broken update (checksums valid, image garbage): every trial boot
        # panics; after three attempts GRUB must return to the good slot.
        bad = os.path.join(tmp, "bad")
        os.makedirs(bad)
        subprocess.run(["tar", "-xf", bundle, "-C", bad, "bzImage"], check=True)
        with open(os.path.join(bad, "system.squashfs"), "wb") as f:
            f.write(os.urandom(4 * 1024 * 1024))
        with open(os.path.join(bad, "VERSION"), "w") as f:
            f.write("broken-test\n")
        sums = subprocess.run(["sha256sum", "system.squashfs", "bzImage", "VERSION"], cwd=bad,
                              capture_output=True, text=True, check=True).stdout
        with open(os.path.join(bad, "SHA256SUMS"), "w") as f:
            f.write(sums)
        bad_tar = subprocess.run(["tar", "-cf", "-", "-C", bad, "system.squashfs", "bzImage", "VERSION", "SHA256SUMS"],
                                 capture_output=True, check=True).stdout
        rc, out, err = ssh("cat > /data/home/admin/bad.tar", stdin=bad_tar)
        rc, out, err = ssh(f"echo '{PASSWORD}' | sudo -S -p '' nexus system update /data/home/admin/bad.tar --json", timeout=300)
        res.check("broken update written to slot a", rc == 0 and '"slot": "a"' in out, out + err)
        ssh(f"echo '{PASSWORD}' | sudo -S -p '' nexus system reboot", timeout=30)
        for attempt in range(3):
            vm.expect(r"Kernel panic", 180)
        vm.expect(r"login:", 240)
        wait_ssh()
        rc, out, err = ssh(f"echo '{PASSWORD}' | sudo -S -p '' nexus system slot --json")
        try:
            slot = json.loads(out)
        except json.JSONDecodeError:
            slot = {}
        res.check("failed update falls back to the good slot after 3 attempts",
                  slot.get("booted_slot") == "b" and slot.get("good_slot") == "b" and slot.get("pending_slot") == "",
                  out + err)
        ok = any(http("/")[0] == 200 or time.sleep(2) for _ in range(30))
        res.check("project running after update fallback", ok)
    else:
        res.check("update bundle present", False, bundle)

    # ------------------------------------------------------------ clean shutdown
    vm.monitor("system_powerdown")
    vm.expect(r"Remounting /data read-only\s+\[ OK \]", 90)
    code = vm.wait_exit(60)
    res.check("ACPI power button: clean shutdown", code == 0, f"exit {code}")
finally:
    vm.kill()

# ---------------------------------------------------------------- BIOS boot
vm = boot_disk("e2e-bios", uefi=False)
try:
    vm.expect(r"login:", 180)
    res.check("installed disk boots with legacy BIOS", True)
    wait_ssh()
    rc, out, _ = ssh("nexus projects --json")
    res.check("projects intact after BIOS boot", '"hello"' in out, out[:200])
finally:
    vm.kill()

shutil.rmtree(tmp, ignore_errors=True)
res.finish()
