//! `nexus system ...` — host configuration and A/B updates. Requires root.
//!
//! A/B layout (see documentation/updates.md): partitions NEXUS_A and NEXUS_B
//! hold read-only squashfs system images; kernels live on the EFI system
//! partition (NEXUS_ESP) under /nexus/<slot>/bzImage; GRUB reads the boot
//! state from /grub/grubenv:
//!   nexus_slot   known-good slot (a|b)
//!   nexus_try    slot to try on next boot ("" when none)
//!   nexus_tries  remaining attempts for nexus_try (GRUB decrements)
//! A trial boot is confirmed by `nexus system confirm-boot` (run by init
//! once the system is healthy); otherwise GRUB falls back to nexus_slot.

use crate::args::Args;
use crate::{Fail, R, print_json};
use nexus_core::schema;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const ESP_MNT: &str = "/run/nexus/esp";

fn need_root() -> R {
    if unsafe { libc::geteuid() } != 0 {
        return Err(Fail::Usage(
            "this command must run as root (use sudo)".into(),
        ));
    }
    Ok(())
}

fn sh(cmd: &str, args: &[&str]) -> Result<String, Fail> {
    let o = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| Fail::Local(format!("cannot run {cmd}: {e}")))?;
    if !o.status.success() {
        return Err(Fail::Local(format!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

fn valid_hostname(h: &str) -> bool {
    let b = h.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn kill_pidfile(path: &str) {
    if let Some(pid) = fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
    {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

fn kill_by_name(name: &str) {
    if let Ok(rd) = fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            if fs::read_to_string(e.path().join("comm"))
                .map(|c| c.trim() == name)
                .unwrap_or(false)
            {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
    }
}

fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let real = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = real.with_extension("nexus-tmp");
    fs::write(&tmp, content)?;
    fs::rename(tmp, real)
}

struct Esp;

impl Esp {
    fn mount() -> Result<Esp, Fail> {
        let dev = sh("blkid", &["-L", "NEXUS_ESP"])?.trim().to_string();
        if dev.is_empty() {
            return Err(Fail::Local(
                "EFI system partition NEXUS_ESP not found".into(),
            ));
        }
        fs::create_dir_all(ESP_MNT)?;
        let _ = Command::new("umount")
            .arg(ESP_MNT)
            .stderr(std::process::Stdio::null())
            .status();
        sh(
            "mount",
            &["-t", "vfat", "-o", "rw,nosuid,nodev,noexec", &dev, ESP_MNT],
        )?;
        Ok(Esp)
    }
    fn path(&self, rel: &str) -> PathBuf {
        Path::new(ESP_MNT).join(rel)
    }
}

impl Drop for Esp {
    fn drop(&mut self) {
        let _ = Command::new("sync").status();
        let _ = Command::new("umount").arg(ESP_MNT).status();
    }
}

fn grubenv(esp: &Esp) -> PathBuf {
    esp.path("grub/grubenv")
}

fn env_list(esp: &Esp) -> Result<Vec<(String, String)>, Fail> {
    let out = sh("grub-editenv", &[&grubenv(esp).to_string_lossy(), "list"])?;
    Ok(out
        .lines()
        .filter_map(|l| {
            l.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect())
}

fn env_set(esp: &Esp, kv: &[(&str, &str)]) -> R {
    let file = grubenv(esp).to_string_lossy().into_owned();
    let mut args: Vec<String> = vec![file, "set".into()];
    args.extend(kv.iter().map(|(k, v)| format!("{k}={v}")));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    sh("grub-editenv", &refs)?;
    Ok(())
}

fn current_slot() -> String {
    fs::read_to_string("/run/nexus/slot")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn cmdline_has(flag: &str) -> bool {
    fs::read_to_string("/proc/cmdline")
        .map(|c| c.split_whitespace().any(|w| w == flag))
        .unwrap_or(false)
}

pub fn run(a: &Args, json: bool) -> R {
    let sub = a
        .word(1)
        .ok_or_else(|| Fail::Usage("system needs a subcommand".into()))?;
    let done = |v: serde_json::Value, text: String| {
        if json {
            print_json(&schema::envelope(&format!("system.{sub}"), v));
        } else {
            println!("{text}");
        }
    };
    match sub {
        "hostname" => {
            need_root()?;
            let h = a.need(2, "hostname").map_err(Fail::Usage)?.to_lowercase();
            if !valid_hostname(&h) {
                return Err(Fail::Usage(
                    "hostname: 1-63 chars of a-z, 0-9 and '-'".into(),
                ));
            }
            atomic_write(Path::new("/etc/hostname"), &format!("{h}\n"))?;
            atomic_write(
                Path::new("/etc/hosts"),
                &format!(
                    "127.0.0.1\tlocalhost\n127.0.1.1\t{h} {h}.local\n::1\tlocalhost ip6-localhost\n"
                ),
            )?;
            sh("hostname", &[&h])?;
            kill_by_name("mdnsd");
            done(
                json!({ "hostname": h }),
                format!("hostname set to {h} (reachable as {h}.local)"),
            );
            Ok(())
        }
        "network" => {
            need_root()?;
            let mode = a.need(2, "dhcp or static").map_err(Fail::Usage)?;
            let conf = match mode {
                "dhcp" => "MODE=dhcp\nINTERFACE=auto\n".to_string(),
                "static" => {
                    let addr = a
                        .get("address")
                        .ok_or_else(|| Fail::Usage("--address A.B.C.D/P required".into()))?;
                    let gw = a.get("gateway").unwrap_or("");
                    let dns = a.get("dns").unwrap_or("1.1.1.1 9.9.9.9");
                    let ok_ip = |s: &str| {
                        s.split('/')
                            .next()
                            .and_then(|ip| ip.parse::<std::net::IpAddr>().ok())
                            .is_some()
                    };
                    if !ok_ip(addr)
                        || !addr.contains('/')
                        || (!gw.is_empty() && !ok_ip(gw))
                        || dns.split_whitespace().any(|d| !ok_ip(d))
                    {
                        return Err(Fail::Usage("invalid address, gateway or dns".into()));
                    }
                    let iface = a.get("interface").unwrap_or("auto");
                    if !iface
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
                    {
                        return Err(Fail::Usage("invalid interface name".into()));
                    }
                    format!(
                        "MODE=static\nINTERFACE={iface}\nADDRESS={addr}\nGATEWAY={gw}\nDNS=\"{dns}\"\n"
                    )
                }
                _ => return Err(Fail::Usage("network mode must be dhcp or static".into())),
            };
            atomic_write(
                Path::new("/data/etc/nexus/network.conf"),
                &format!("# written by nexus system network\n{conf}"),
            )?;
            kill_pidfile("/run/nexus/netd.pid");
            done(
                json!({ "network": mode }),
                format!(
                    "network set to {mode}; reconfiguring now (SSH sessions may drop if the address changes)"
                ),
            );
            Ok(())
        }
        "slot" => {
            need_root()?;
            let esp = Esp::mount()?;
            let env = env_list(&esp)?;
            let get = |k: &str| {
                env.iter()
                    .find(|(x, _)| x == k)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            };
            let v = json!({
                "booted_slot": current_slot(),
                "trial_boot": cmdline_has("nexus.trial=1"),
                "good_slot": get("nexus_slot"),
                "pending_slot": get("nexus_try"),
                "pending_tries": get("nexus_tries"),
            });
            done(
                v.clone(),
                format!(
                    "booted slot: {}{}\nknown-good slot: {}\npending update: {}",
                    v["booted_slot"].as_str().unwrap_or("?"),
                    if v["trial_boot"] == true {
                        " (trial boot, not yet confirmed)"
                    } else {
                        ""
                    },
                    v["good_slot"].as_str().unwrap_or("?"),
                    if get("nexus_try").is_empty() {
                        "none".to_string()
                    } else {
                        format!(
                            "slot {} ({} tries left)",
                            get("nexus_try"),
                            get("nexus_tries")
                        )
                    }
                ),
            );
            Ok(())
        }
        "confirm-boot" => {
            need_root()?;
            let slot = current_slot();
            if slot.is_empty() {
                return Err(Fail::Local("not booted from an installed slot".into()));
            }
            let esp = Esp::mount()?;
            let env = env_list(&esp)?;
            let good = env
                .iter()
                .find(|(k, _)| k == "nexus_slot")
                .map(|(_, v)| v.as_str())
                == Some(slot.as_str());
            let pending = env.iter().any(|(k, v)| k == "nexus_try" && !v.is_empty());
            if good && !pending {
                done(
                    json!({ "slot": slot, "changed": false }),
                    format!("slot {slot} already confirmed"),
                );
                return Ok(());
            }
            env_set(
                &esp,
                &[
                    ("nexus_slot", &slot),
                    ("nexus_try", ""),
                    ("nexus_tries", "0"),
                ],
            )?;
            done(
                json!({ "slot": slot, "changed": true }),
                format!("slot {slot} confirmed as the known-good system"),
            );
            Ok(())
        }
        "update" => update(a, json),
        "reboot" | "poweroff" => {
            need_root()?;
            let _ = Command::new("sync").status();
            let e = Command::new(format!("/sbin/{sub}")).status();
            e.map(|_| ()).map_err(|e| Fail::Local(e.to_string()))
        }
        other => Err(Fail::Usage(format!("unknown system subcommand {other:?}"))),
    }
}

fn update(a: &Args, json: bool) -> R {
    need_root()?;
    let bundle = a.need(2, "update bundle path").map_err(Fail::Usage)?;
    let slot = current_slot();
    let target = match slot.as_str() {
        "a" => "b",
        "b" => "a",
        _ => {
            return Err(Fail::Local(
                "not booted from an installed slot; cannot update".into(),
            ));
        }
    };
    let staging = PathBuf::from("/data/nexus/update-staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let st = staging.to_string_lossy().into_owned();
    let say = |m: &str| {
        if !json {
            eprintln!("==> {m}");
        }
    };
    say("unpacking bundle");
    sh("tar", &["-xf", bundle, "-C", &st])?;
    say("verifying checksums");
    let o = Command::new("sha256sum")
        .args(["-c", "SHA256SUMS"])
        .current_dir(&staging)
        .output()?;
    if !o.status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(Fail::Local(format!(
            "checksum verification failed:\n{}",
            String::from_utf8_lossy(&o.stdout)
        )));
    }
    for f in ["system.squashfs", "bzImage", "VERSION"] {
        let listed = fs::read_to_string(staging.join("SHA256SUMS")).unwrap_or_default();
        if !listed
            .lines()
            .any(|l| l.split_whitespace().nth(1) == Some(f))
        {
            return Err(Fail::Local(format!(
                "bundle does not cover {f} in SHA256SUMS"
            )));
        }
    }
    let version = fs::read_to_string(staging.join("VERSION"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let label = format!("NEXUS_{}", target.to_uppercase());
    let dev = sh(
        "blkid",
        &["-l", "-o", "device", "-t", &format!("PARTLABEL={label}")],
    )?
    .trim()
    .to_string();
    if dev.is_empty() {
        return Err(Fail::Local(format!("partition {label} not found")));
    }
    let img = staging.join("system.squashfs");
    let img_size = fs::metadata(&img)?.len();
    let part_size: u64 = sh("blockdev", &["--getsize64", &dev])
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if part_size > 0 && img_size > part_size {
        return Err(Fail::Local(format!(
            "image ({img_size} bytes) does not fit {label} ({part_size} bytes)"
        )));
    }
    say(&format!(
        "writing system image {version} to slot {target} ({dev})"
    ));
    sh(
        "dd",
        &[
            &format!("if={}", img.display()),
            &format!("of={dev}"),
            "bs=1M",
            "conv=fsync",
        ],
    )?;
    say("verifying written image");
    let want = sh("sha256sum", &[&img.to_string_lossy()])?
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    let blocks = img_size.div_ceil(1024 * 1024);
    let got = Command::new("sh")
        .arg("-c")
        .arg(format!("head -c {img_size} {dev} | sha256sum"))
        .output()?;
    let got = String::from_utf8_lossy(&got.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    let _ = blocks;
    if got != want {
        return Err(Fail::Local(
            "read-back verification of the written image failed".into(),
        ));
    }
    let esp = Esp::mount()?;
    let kdir = esp.path(&format!("nexus/{target}"));
    fs::create_dir_all(&kdir)?;
    fs::copy(staging.join("bzImage"), kdir.join("bzImage.new"))?;
    fs::rename(kdir.join("bzImage.new"), kdir.join("bzImage"))?;
    env_set(&esp, &[("nexus_try", target), ("nexus_tries", "3")])?;
    drop(esp);
    let _ = fs::remove_dir_all(&staging);
    let v = json!({ "version": version, "slot": target, "activation": "on next reboot (trial boot, automatic fallback)" });
    if json {
        print_json(&schema::envelope("system.update", v));
    } else {
        println!(
            "NEXUS OS {version} installed to slot {target}.\nReboot to try it: sudo nexus system reboot\nIf the new system does not confirm itself healthy, the next boot returns to slot {slot}."
        );
    }
    if a.has("reboot") {
        let _ = Command::new("/sbin/reboot").status();
    }
    Ok(())
}
