"""Minimal QEMU serial-console driver for NEXUS OS tests (stdlib only)."""
import os
import re
import select
import shutil
import socket
import subprocess
import sys
import time

TOP = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
WORK = os.path.join(TOP, "build", "qemu")
os.makedirs(WORK, exist_ok=True)


def ovmf_args(vars_path):
    for d in ["/usr/share/edk2/x64", "/usr/share/OVMF", "/usr/share/ovmf/x64",
              "/usr/share/edk2-ovmf/x64", "/usr/share/qemu"]:
        for code in ["OVMF_CODE.4m.fd", "OVMF_CODE.fd"]:
            p = os.path.join(d, code)
            if os.path.exists(p):
                if not os.path.exists(vars_path):
                    shutil.copy(os.path.join(d, code.replace("CODE", "VARS")), vars_path)
                return ["-drive", f"if=pflash,format=raw,readonly=on,file={p}",
                        "-drive", f"if=pflash,format=raw,file={vars_path}"]
    raise SystemExit("OVMF firmware not found")


def base_args(mem=512, ssh_port=None, app_port=None):
    a = ["qemu-system-x86_64", "-machine", "q35", "-m", str(mem), "-smp", "2",
         "-display", "none", "-serial", "stdio",
         "-device", "i6300esb",
         "-object", "rng-random,id=rng0,filename=/dev/urandom",
         "-device", "virtio-rng-pci,rng=rng0"]
    if os.access("/dev/kvm", os.W_OK):
        a += ["-enable-kvm", "-cpu", "host"]
    else:
        a += ["-cpu", "max"]
    fwd = ""
    if ssh_port:
        fwd += f",hostfwd=tcp:127.0.0.1:{ssh_port}-:22"
    if app_port:
        fwd += f",hostfwd=tcp:127.0.0.1:{app_port}-:8080"
    a += ["-netdev", f"user,id=n0{fwd}", "-device", "virtio-net-pci,netdev=n0"]
    return a


class Vm:
    def __init__(self, args, name):
        self.name = name
        self.monitor_path = os.path.join(WORK, f"{name}.monitor")
        if os.path.exists(self.monitor_path):
            os.unlink(self.monitor_path)
        self.log = open(os.path.join(WORK, f"{name}.log"), "ab")
        self.buf = ""
        self.proc = subprocess.Popen(
            args + ["-monitor", f"unix:{self.monitor_path},server,nowait"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        self.started = time.time()

    def _read(self, timeout):
        r, _, _ = select.select([self.proc.stdout], [], [], timeout)
        if not r:
            return False
        data = os.read(self.proc.stdout.fileno(), 65536)
        if not data:
            raise EOFError(f"{self.name}: QEMU exited (code {self.proc.poll()})")
        self.log.write(data)
        self.log.flush()
        self.buf += data.decode("utf-8", "replace")
        return True

    def expect(self, pattern, timeout=60):
        """Wait for regex `pattern`; consume output up to the match."""
        rx = re.compile(pattern, re.M | re.S) if "(.*?)" in pattern else re.compile(pattern, re.M)
        deadline = time.time() + timeout
        while True:
            m = rx.search(self.buf)
            if m:
                self.buf = self.buf[m.end():]
                return m
            left = deadline - time.time()
            if left <= 0:
                tail = self.buf[-1500:]
                raise TimeoutError(f"{self.name}: timeout waiting for {pattern!r}\n--- last output ---\n{tail}")
            self._read(min(left, 1.0))

    def send(self, text):
        self.proc.stdin.write(text.encode())
        self.proc.stdin.flush()

    def sendline(self, text=""):
        self.send(text + "\n")

    def run(self, cmd, timeout=60, prompt=r"[#$] $"):
        """Run a shell command on the serial console; return its output."""
        n = int(time.time() * 1000) % 100000
        self.sendline(f"echo __S{n}$((0))__; {cmd}; echo __E{n}$((0))__ $?")
        m = self.expect(rf"__S{n}0__\r?\n(.*?)__E{n}0__ (\d+)", timeout)
        out, rc = m.group(1), int(m.group(2))
        # Drop console noise from daemons (kernel/udhcpc/ntpd messages).
        noise = ("udhcpc:", "ntpd:", "deleting routers", "adding dns", "[ ")
        lines = [l for l in out.replace("\r", "").split("\n") if not l.startswith(noise)]
        return "\n".join(lines), rc

    def monitor(self, command):
        for _ in range(50):
            if os.path.exists(self.monitor_path):
                break
            time.sleep(0.1)
        s = socket.socket(socket.AF_UNIX)
        s.connect(self.monitor_path)
        s.recv(4096)
        s.sendall((command + "\n").encode())
        time.sleep(0.3)
        s.close()

    def wait_exit(self, timeout=60):
        deadline = time.time() + timeout
        while self.proc.poll() is None:
            if time.time() > deadline:
                raise TimeoutError(f"{self.name}: did not power off")
            try:
                self._read(0.5)
            except EOFError:
                break
        return self.proc.wait()

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait()


class Results:
    def __init__(self, name):
        self.name, self.passed, self.failed, self.metrics = name, 0, 0, {}

    def check(self, label, cond, detail=""):
        if cond:
            self.passed += 1
            print(f"PASS: {label}", flush=True)
        else:
            self.failed += 1
            print(f"FAIL: {label} {detail}", flush=True)
        return cond

    def finish(self):
        import json
        with open(os.path.join(WORK, f"{self.name}-results.json"), "w") as f:
            json.dump({"passed": self.passed, "failed": self.failed, "metrics": self.metrics}, f, indent=2)
        print(f"\n{self.name}: {self.passed} passed, {self.failed} failed")
        for k, v in self.metrics.items():
            print(f"  {k}: {v}")
        sys.exit(1 if self.failed else 0)
