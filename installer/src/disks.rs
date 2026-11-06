//! Hardware and disk discovery from /proc and /sys.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Smallest disk NEXUS installs to (ESP + two system slots + data).
pub const MIN_BYTES: u64 = 4 << 30;

pub struct Hardware {
    pub cpu: String,
    pub cores: usize,
    pub mem_mib: u64,
    pub uefi: bool,
}

pub fn hardware() -> Hardware {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpuinfo
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_else(|| "unknown CPU".into());
    let cores = cpuinfo
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count();
    let meminfo = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mem_mib = nexus_core::resources::meminfo_value(&meminfo, "MemTotal").unwrap_or(0) >> 20;
    Hardware {
        cpu,
        cores,
        mem_mib,
        uefi: Path::new("/sys/firmware/efi").exists(),
    }
}

#[derive(Debug, Clone)]
pub struct Disk {
    pub name: String,
    pub size: u64,
    pub model: String,
    pub transport: String,
    pub removable: bool,
    /// Existing partitions: "sda1 ext4 \"label\"".
    pub partitions: Vec<String>,
}

impl Disk {
    pub fn describe(&self) -> String {
        format!(
            "{:<10} {:>8}  {} ({}{})",
            self.name,
            human_gb(self.size),
            if self.model.is_empty() {
                "unknown model"
            } else {
                &self.model
            },
            self.transport,
            if self.removable { ", removable" } else { "" }
        )
    }
}

/// Decimal units, as printed on disk labels ("256 GB").
pub fn human_gb(b: u64) -> String {
    let gb = b as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else if gb >= 10.0 {
        format!("{gb:.0} GB")
    } else {
        format!("{gb:.1} GB")
    }
}

fn read(p: impl AsRef<Path>) -> String {
    fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn transport(name: &str, sys: &Path) -> String {
    let link = fs::canonicalize(sys.join("device"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    if link.contains("/usb") {
        "USB".into()
    } else if name.starts_with("nvme") {
        "NVMe".into()
    } else if name.starts_with("mmcblk") {
        "eMMC/SD".into()
    } else if name.starts_with("vd") || link.contains("virtio") {
        "virtio".into()
    } else {
        "SATA/SCSI".into()
    }
}

/// `blkid` TYPE and LABEL of a block device.
pub fn fs_info(dev: &str) -> (String, String) {
    let out = Command::new("blkid")
        .args(["-o", "export", dev])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let get = |k: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .unwrap_or("")
            .to_string()
    };
    (get("TYPE"), get("LABEL"))
}

pub fn partitions_of(name: &str) -> Vec<String> {
    let sys = Path::new("/sys/block").join(name);
    let mut parts: Vec<String> = fs::read_dir(&sys)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|p| p.starts_with(name) && sys.join(p).join("partition").exists())
                .collect()
        })
        .unwrap_or_default();
    parts.sort();
    parts
}

/// Disks suitable for installation, excluding the installer media.
pub fn eligible(exclude: Option<&str>) -> Vec<Disk> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/sys/block") else {
        return out;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for name in names {
        if ["loop", "ram", "zram", "sr", "dm-", "md", "fd"]
            .iter()
            .any(|p| name.starts_with(p))
            || Some(name.as_str()) == exclude
        {
            continue;
        }
        let sys = Path::new("/sys/block").join(&name);
        if read(sys.join("ro")) == "1" {
            continue;
        }
        let size = read(sys.join("size")).parse::<u64>().unwrap_or(0) * 512;
        if size < MIN_BYTES {
            continue;
        }
        let partitions = partitions_of(&name)
            .into_iter()
            .map(|p| {
                let (t, l) = fs_info(&format!("/dev/{p}"));
                let mut s = p.clone();
                if !t.is_empty() {
                    s.push_str(&format!(" {t}"));
                }
                if !l.is_empty() {
                    s.push_str(&format!(" \"{l}\""));
                }
                s
            })
            .collect();
        out.push(Disk {
            model: read(sys.join("device/model"))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
            removable: read(sys.join("removable")) == "1",
            transport: transport(&name, &sys),
            name,
            size,
            partitions,
        });
    }
    out
}

/// Device node for partition `n` of `disk` (sda -> sda1, nvme0n1 -> nvme0n1p1).
pub fn part_dev(disk: &str, n: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("/dev/{disk}p{n}")
    } else {
        format!("/dev/{disk}{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_names() {
        assert_eq!(part_dev("sda", 1), "/dev/sda1");
        assert_eq!(part_dev("vda", 5), "/dev/vda5");
        assert_eq!(part_dev("nvme0n1", 2), "/dev/nvme0n1p2");
        assert_eq!(part_dev("mmcblk0", 3), "/dev/mmcblk0p3");
    }

    #[test]
    fn sizes() {
        assert_eq!(human_gb(256_060_514_304), "256 GB");
        assert_eq!(human_gb(8_000_000_000), "8.0 GB");
        assert_eq!(human_gb(2_000_398_934_016), "2.0 TB");
    }
}
