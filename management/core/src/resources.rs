//! Hardware resource metrics from /proc and /sys.
//!
//! Policy: never invent numbers. Anything that cannot be measured on the
//! current hardware is `null` in JSON and NOT DETECTED / NOT SUPPORTED in
//! human output. Sampling is on demand only (no background polling).

use serde::{Deserialize, Serialize};
use std::ffi::CStr;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CpuTimes {
    pub total: u64,
    pub idle: u64,
}

/// Index 0 is the aggregate line, then one entry per CPU.
pub fn parse_proc_stat(text: &str) -> Vec<CpuTimes> {
    text.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let v: Vec<u64> = l
                .split_whitespace()
                .skip(1)
                .filter_map(|x| x.parse().ok())
                .collect();
            // user nice system idle iowait irq softirq steal (guest is in user)
            let total: u64 = v.iter().take(8).sum();
            let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
            CpuTimes { total, idle }
        })
        .collect()
}

pub fn usage_percent(prev: CpuTimes, cur: CpuTimes) -> f64 {
    let dt = cur.total.saturating_sub(prev.total);
    if dt == 0 {
        return 0.0;
    }
    let di = cur.idle.saturating_sub(prev.idle);
    round1(100.0 * (dt.saturating_sub(di)) as f64 / dt as f64)
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetCounters {
    pub iface: String,
    pub rx: u64,
    pub tx: u64,
}

pub fn parse_net_dev(text: &str) -> Vec<NetCounters> {
    text.lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let v: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            Some(NetCounters {
                iface: name.trim().to_string(),
                rx: *v.first()?,
                tx: *v.get(8)?,
            })
        })
        .collect()
}

/// `/proc/meminfo` value in bytes.
pub fn meminfo_value(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| {
        let rest = l.strip_prefix(key)?.strip_prefix(':')?;
        rest.split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    })
}

/// A point-in-time reading used to compute rates between two samples.
#[derive(Debug, Clone)]
pub struct Sample {
    pub at: Instant,
    pub cpu: Vec<CpuTimes>,
    pub net: Vec<NetCounters>,
}

impl Sample {
    pub fn take() -> Sample {
        Sample {
            at: Instant::now(),
            cpu: parse_proc_stat(&fs::read_to_string("/proc/stat").unwrap_or_default()),
            net: parse_net_dev(&fs::read_to_string("/proc/net/dev").unwrap_or_default()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cpu {
    pub model: Option<String>,
    pub cores: usize,
    pub usage_percent: f64,
    pub per_core_percent: Vec<f64>,
    pub available_percent: f64,
    pub load_average: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Swap {
    pub configured: bool,
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Memory {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub cached_bytes: u64,
    pub used_percent: f64,
    pub swap: Swap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Storage {
    pub mount: String,
    pub device: String,
    pub filesystem: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
    pub used_percent: f64,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskHealth {
    pub device: String,
    pub model: Option<String>,
    pub size_bytes: u64,
    /// "PASSED", "FAILED", "NOT SUPPORTED" or "NOT CHECKED".
    pub health: String,
    pub temperature_celsius: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetInterface {
    pub name: String,
    pub state: String,
    pub mac: Option<String>,
    pub ipv4: Vec<String>,
    pub ipv6: Vec<String>,
    pub speed_mbps: Option<u64>,
    pub rx_bytes_per_sec: Option<f64>,
    pub tx_bytes_per_sec: Option<f64>,
    pub rx_bytes_total: u64,
    pub tx_bytes_total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Gpu {
    pub pci_address: String,
    pub vendor: String,
    pub vendor_id: String,
    pub device_id: String,
    pub driver: Option<String>,
    pub utilization_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
    /// "SUPPORTED" when utilisation can be read, else "NOT SUPPORTED".
    pub metrics: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Connections {
    pub tcp_established: u64,
    pub tcp_listening: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Resources {
    pub schema_version: u32,
    pub hostname: String,
    pub os_version: String,
    pub kernel: String,
    pub uptime_seconds: u64,
    pub cpu: Cpu,
    pub memory: Memory,
    pub storage: Vec<Storage>,
    pub disks: Vec<DiskHealth>,
    pub network: Vec<NetInterface>,
    pub connections: Connections,
    /// Empty when no GPU is detected.
    pub gpus: Vec<Gpu>,
}

pub fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

pub fn uptime_seconds() -> u64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split('.').next().and_then(|x| x.parse().ok()))
        .unwrap_or(0)
}

pub fn os_version() -> String {
    let text = fs::read_to_string("/etc/nexus-release").unwrap_or_default();
    text.lines()
        .find_map(|l| l.strip_prefix("NEXUS_VERSION="))
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| format!("{} (host)", crate::VERSION))
}

fn read_trim(p: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

pub fn cpu_model() -> Option<String> {
    let text = fs::read_to_string("/proc/cpuinfo").ok()?;
    text.lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

pub fn load_average() -> [f64; 3] {
    let s = fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let v: Vec<f64> = s
        .split_whitespace()
        .take(3)
        .filter_map(|x| x.parse().ok())
        .collect();
    [
        v.first().copied().unwrap_or(0.0),
        v.get(1).copied().unwrap_or(0.0),
        v.get(2).copied().unwrap_or(0.0),
    ]
}

pub fn memory() -> Memory {
    let t = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let total = meminfo_value(&t, "MemTotal").unwrap_or(0);
    let avail = meminfo_value(&t, "MemAvailable").unwrap_or(0);
    let cached =
        meminfo_value(&t, "Cached").unwrap_or(0) + meminfo_value(&t, "Buffers").unwrap_or(0);
    let st = meminfo_value(&t, "SwapTotal").unwrap_or(0);
    let sf = meminfo_value(&t, "SwapFree").unwrap_or(0);
    let used = total.saturating_sub(avail);
    Memory {
        total_bytes: total,
        used_bytes: used,
        available_bytes: avail,
        cached_bytes: cached,
        used_percent: if total > 0 {
            round1(100.0 * used as f64 / total as f64)
        } else {
            0.0
        },
        swap: Swap {
            configured: st > 0,
            total_bytes: st,
            used_bytes: st.saturating_sub(sf),
        },
    }
}

pub fn statvfs(path: &str) -> Option<(u64, u64, u64, bool)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    let total = s.f_blocks as u64 * f;
    let free = s.f_bfree as u64 * f;
    let avail = s.f_bavail as u64 * f;
    Some((total, total - free, avail, s.f_flag & libc::ST_RDONLY != 0))
}

pub fn storage() -> Vec<Storage> {
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    let mut out: Vec<Storage> = Vec::new();
    for l in mounts.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        let (dev, mnt, fs_type) = (f[0], f[1].replace("\\040", " "), f[2]);
        let interesting = dev.starts_with("/dev/") || mnt == "/data";
        if !interesting
            || out
                .iter()
                .any(|s| s.device == dev && dev.starts_with("/dev/"))
        {
            continue;
        }
        if let Some((total, used, avail, ro)) = statvfs(&mnt) {
            if total == 0 {
                continue;
            }
            out.push(Storage {
                mount: mnt,
                device: dev.to_string(),
                filesystem: fs_type.to_string(),
                total_bytes: total,
                used_bytes: used,
                available_bytes: avail,
                used_percent: round1(100.0 * used as f64 / total as f64),
                read_only: ro,
            });
        }
    }
    out
}

/// Whole disks from /sys/block. With `check_health`, SMART status is read
/// via smartctl (spins up disks — only on explicit request).
pub fn disks(check_health: bool) -> Vec<DiskHealth> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/sys/block") else {
        return out;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            !["loop", "ram", "zram", "sr", "dm-", "md"]
                .iter()
                .any(|p| n.starts_with(p))
        })
        .collect();
    names.sort();
    for n in names {
        let base = Path::new("/sys/block").join(&n);
        let sectors: u64 = read_trim(base.join("size"))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if sectors == 0 {
            continue;
        }
        let model = read_trim(base.join("device/model")).filter(|s| !s.is_empty());
        let (health, temp) = if check_health {
            smart_health(&format!("/dev/{n}"))
        } else {
            ("NOT CHECKED".to_string(), None)
        };
        out.push(DiskHealth {
            device: format!("/dev/{n}"),
            model,
            size_bytes: sectors * 512,
            health,
            temperature_celsius: temp,
        });
    }
    out
}

fn smart_health(dev: &str) -> (String, Option<i64>) {
    let out = std::process::Command::new("smartctl")
        .args(["-H", "-A", "-j", dev])
        .stderr(std::process::Stdio::null())
        .output();
    let Ok(out) = out else {
        return ("NOT SUPPORTED".into(), None);
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return ("NOT SUPPORTED".into(), None);
    };
    let temp = v["temperature"]["current"].as_i64();
    match v["smart_status"]["passed"].as_bool() {
        Some(true) => ("PASSED".into(), temp),
        Some(false) => ("FAILED".into(), temp),
        None => ("NOT SUPPORTED".into(), temp),
    }
}

/// (ipv4, ipv6) address strings with prefix length per interface.
fn interface_addresses() -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        cur = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
            continue;
        }
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let family = unsafe { (*ifa.ifa_addr).sa_family } as i32;
        let prefix = |mask: *const libc::sockaddr, v6: bool| -> u32 {
            if mask.is_null() {
                return 0;
            }
            unsafe {
                if v6 {
                    let m = &*(mask as *const libc::sockaddr_in6);
                    m.sin6_addr.s6_addr.iter().map(|b| b.count_ones()).sum()
                } else {
                    let m = &*(mask as *const libc::sockaddr_in);
                    m.sin_addr.s_addr.count_ones()
                }
            }
        };
        if family == libc::AF_INET {
            let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
            let ip = std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
            out.push((
                name,
                format!("{ip}/{}", prefix(ifa.ifa_netmask, false)),
                false,
            ));
        } else if family == libc::AF_INET6 {
            let a = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
            let ip = std::net::Ipv6Addr::from(a.sin6_addr.s6_addr);
            out.push((
                name,
                format!("{ip}/{}", prefix(ifa.ifa_netmask, true)),
                true,
            ));
        }
    }
    unsafe { libc::freeifaddrs(ifap) };
    out
}

pub fn network(prev: Option<&Sample>, cur: &Sample) -> Vec<NetInterface> {
    let addrs = interface_addresses();
    let mut out = Vec::new();
    for c in &cur.net {
        if c.iface == "lo" {
            continue;
        }
        let base = Path::new("/sys/class/net").join(&c.iface);
        let rates = prev.and_then(|p| {
            let pc = p.net.iter().find(|x| x.iface == c.iface)?;
            let dt = cur.at.duration_since(p.at).as_secs_f64();
            (dt > 0.0).then(|| {
                (
                    (c.rx.saturating_sub(pc.rx)) as f64 / dt,
                    (c.tx.saturating_sub(pc.tx)) as f64 / dt,
                )
            })
        });
        out.push(NetInterface {
            name: c.iface.clone(),
            state: read_trim(base.join("operstate")).unwrap_or_else(|| "unknown".into()),
            mac: read_trim(base.join("address")),
            ipv4: addrs
                .iter()
                .filter(|a| a.0 == c.iface && !a.2)
                .map(|a| a.1.clone())
                .collect(),
            ipv6: addrs
                .iter()
                .filter(|a| a.0 == c.iface && a.2)
                .map(|a| a.1.clone())
                .collect(),
            speed_mbps: read_trim(base.join("speed"))
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|s| *s > 0)
                .map(|s| s as u64),
            rx_bytes_per_sec: rates.map(|r| r.0.round()),
            tx_bytes_per_sec: rates.map(|r| r.1.round()),
            rx_bytes_total: c.rx,
            tx_bytes_total: c.tx,
        });
    }
    out
}

/// Count TCP sockets by state from /proc/net/tcp{,6}.
pub fn connections() -> Connections {
    let mut est = 0;
    let mut listen = 0;
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for l in fs::read_to_string(f).unwrap_or_default().lines().skip(1) {
            match l.split_whitespace().nth(3) {
                Some("01") => est += 1,
                Some("0A") => listen += 1,
                _ => {}
            }
        }
    }
    Connections {
        tcp_established: est,
        tcp_listening: listen,
    }
}

/// Listening TCP ports (any address) with socket inode.
pub fn listening_tcp_ports() -> Vec<(u16, u64)> {
    let mut out = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for l in fs::read_to_string(f).unwrap_or_default().lines().skip(1) {
            let c: Vec<&str> = l.split_whitespace().collect();
            if c.len() < 10 || c[3] != "0A" {
                continue;
            }
            let port = c[1]
                .rsplit(':')
                .next()
                .and_then(|p| u16::from_str_radix(p, 16).ok());
            if let (Some(port), Ok(inode)) = (port, c[9].parse::<u64>()) {
                out.push((port, inode));
            }
        }
    }
    out
}

/// Find the process owning a socket inode by scanning /proc/*/fd.
pub fn socket_owner(inode: u64) -> Option<(u32, String)> {
    let target = format!("socket:[{inode}]");
    for e in fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(fds) = fs::read_dir(e.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if fs::read_link(fd.path())
                .map(|l| l.to_string_lossy() == target)
                .unwrap_or(false)
            {
                let comm = read_trim(e.path().join("comm")).unwrap_or_default();
                return Some((pid, comm));
            }
        }
    }
    None
}

fn pci_vendor(id: &str) -> &'static str {
    match id {
        "0x8086" => "Intel",
        "0x10de" => "NVIDIA",
        "0x1002" => "AMD",
        "0x1234" => "QEMU/Bochs",
        "0x1af4" => "Red Hat (virtio)",
        "0x15ad" => "VMware",
        "0x1a03" => "ASPEED",
        "0x102b" => "Matrox",
        _ => "Unknown",
    }
}

/// Display controllers (PCI class 0x03xxxx).
pub fn gpus() -> Vec<Gpu> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/sys/bus/pci/devices") else {
        return out;
    };
    let mut devs: Vec<_> = rd.flatten().collect();
    devs.sort_by_key(|e| e.file_name());
    for e in devs {
        let p = e.path();
        let class = read_trim(p.join("class")).unwrap_or_default();
        if !class.starts_with("0x03") {
            continue;
        }
        let vendor_id = read_trim(p.join("vendor")).unwrap_or_default();
        let util = read_trim(p.join("gpu_busy_percent")).and_then(|s| s.parse::<f64>().ok());
        let mem_used = read_trim(p.join("mem_info_vram_used")).and_then(|s| s.parse().ok());
        let mem_total = read_trim(p.join("mem_info_vram_total")).and_then(|s| s.parse().ok());
        out.push(Gpu {
            pci_address: e.file_name().to_string_lossy().into_owned(),
            vendor: pci_vendor(&vendor_id).into(),
            device_id: read_trim(p.join("device")).unwrap_or_default(),
            vendor_id,
            driver: fs::read_link(p.join("driver"))
                .ok()
                .and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned())),
            utilization_percent: util,
            memory_used_bytes: mem_used,
            memory_total_bytes: mem_total,
            metrics: if util.is_some() {
                "SUPPORTED".into()
            } else {
                "NOT SUPPORTED".into()
            },
        });
    }
    out
}

pub fn kernel_release() -> String {
    read_trim("/proc/sys/kernel/osrelease").unwrap_or_default()
}

/// Collect everything. `prev` enables CPU/network rates; without it a short
/// sampling window of `window` is used.
pub fn collect(prev: Option<&Sample>, window: Duration, check_health: bool) -> (Resources, Sample) {
    let owned_prev;
    let prev = match prev {
        Some(p) => p,
        None => {
            owned_prev = Sample::take();
            thread::sleep(window);
            &owned_prev
        }
    };
    let cur = Sample::take();
    let per: Vec<f64> = cur
        .cpu
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, c)| usage_percent(prev.cpu.get(i).copied().unwrap_or_default(), *c))
        .collect();
    let total = usage_percent(
        prev.cpu.first().copied().unwrap_or_default(),
        cur.cpu.first().copied().unwrap_or_default(),
    );
    let res = Resources {
        schema_version: crate::schema::SCHEMA_VERSION,
        hostname: hostname(),
        os_version: os_version(),
        kernel: kernel_release(),
        uptime_seconds: uptime_seconds(),
        cpu: Cpu {
            model: cpu_model(),
            cores: per.len(),
            usage_percent: total,
            per_core_percent: per,
            available_percent: round1(100.0 - total),
            load_average: load_average(),
        },
        memory: memory(),
        storage: storage(),
        disks: disks(check_health),
        network: network(Some(prev), &cur),
        connections: connections(),
        gpus: gpus(),
    };
    (res, cur)
}

pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat() {
        let t = "cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 50 0 50 350 50 0 0 0 0 0\nintr 1\n";
        let v = parse_proc_stat(t);
        assert_eq!(v.len(), 2);
        assert_eq!(
            v[0],
            CpuTimes {
                total: 1000,
                idle: 800
            }
        );
        let later = CpuTimes {
            total: 2000,
            idle: 1300,
        };
        assert_eq!(usage_percent(v[0], later), 50.0);
        assert_eq!(usage_percent(v[0], v[0]), 0.0);
    }

    #[test]
    fn net_dev() {
        let t = "Inter-|   Receive\n face |bytes\n  eth0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0\n    lo: 5 1 0 0 0 0 0 0 5 1 0 0 0 0 0 0\n";
        let v = parse_net_dev(t);
        assert_eq!(
            v[0],
            NetCounters {
                iface: "eth0".into(),
                rx: 1000,
                tx: 2000
            }
        );
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn meminfo() {
        let t = "MemTotal:       1000 kB\nMemAvailable:    250 kB\n";
        assert_eq!(meminfo_value(t, "MemTotal"), Some(1_024_000));
        assert_eq!(meminfo_value(t, "MemFree"), None);
    }

    #[test]
    fn bytes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn collect_on_host() {
        let (r, _) = collect(None, Duration::from_millis(50), false);
        assert!(r.cpu.cores >= 1);
        assert!(r.memory.total_bytes > 0);
    }
}
