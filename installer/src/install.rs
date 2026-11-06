//! The destructive part of installation. Every step logs what it runs
//! (never secrets) and stops at the first failure.
//!
//! Disk layout (GPT):
//!   1  BIOSBOOT    1 MiB    GRUB core image for legacy BIOS boot
//!   2  NEXUS_ESP   128 MiB  FAT32: GRUB (UEFI + BIOS), kernels, grubenv
//!   3  NEXUS_A     1 GiB    system slot A (read-only squashfs)
//!   4  NEXUS_B     1 GiB    system slot B (read-only squashfs)
//!   5  NEXUS_DATA  rest     ext4: configuration, projects, logs

use crate::disks::{self, Disk};
use crate::{Answers, log};
use std::fs;
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const MEDIA_MNT: &str = "/run/nexus/media";
const ESP_MNT: &str = "/run/nexus/target-esp";
const DATA_MNT: &str = "/run/nexus/target-data";
const FACTORY: &str = "/usr/share/nexus/factory/etc";
const ADMIN_UID: u32 = 1000;

pub struct Media {
    pub device: String,
    pub parent_disk: Option<String>,
    pub version: String,
    pub payload: PathBuf,
}

pub struct Summary {
    pub hostname: String,
    pub admin: String,
    pub ip: Option<String>,
    pub fingerprints: Vec<(String, String)>,
}

fn exec(cmd: &str, args: &[&str]) -> Result<String, String> {
    log(&format!("run: {cmd} {}", args.join(" ")));
    let o = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    if !err.trim().is_empty() {
        log(&format!("  stderr: {}", err.trim()));
    }
    if !o.status.success() {
        return Err(format!("{cmd} failed ({}): {}", o.status, err.trim()));
    }
    Ok(out)
}

fn run_stdin(cmd: &str, args: &[&str], input: &str) -> Result<(), String> {
    use std::io::Write;
    log(&format!("run: {cmd} {} <<input>>", args.join(" ")));
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {cmd}: {e}"))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .map_err(|e| e.to_string())?;
    let o = child.wait_with_output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        let err = String::from_utf8_lossy(&o.stderr);
        log(&format!("  stderr: {}", err.trim()));
        return Err(format!("{cmd} failed: {}", err.trim()));
    }
    Ok(())
}

fn whole_disk_of(dev: &str) -> Option<String> {
    let name = Path::new(dev).file_name()?.to_string_lossy().into_owned();
    let sys = fs::canonicalize(Path::new("/sys/class/block").join(&name)).ok()?;
    if sys.join("partition").exists() {
        sys.parent()?
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    } else {
        Some(name)
    }
}

fn mounted(target: &str) -> bool {
    fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .any(|l| l.split_whitespace().nth(1) == Some(target))
}

pub fn find_media() -> Result<Media, String> {
    let out = Command::new("blkid")
        .args(["-t", "LABEL=NEXUS_LIVE", "-o", "device"])
        .output()
        .map_err(|e| e.to_string())?;
    let devs: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(String::from)
        .collect();
    let Some(device) = devs.first().cloned() else {
        return Err("installation media (label NEXUS_LIVE) not found — boot from the NEXUS ISO or USB stick".into());
    };
    fs::create_dir_all(MEDIA_MNT).map_err(|e| e.to_string())?;
    if !mounted(MEDIA_MNT) {
        exec("mount", &["-t", "iso9660", "-o", "ro", &device, MEDIA_MNT])?;
    }
    let payload = Path::new(MEDIA_MNT).join("nexus");
    if !payload.join("SHA256SUMS").exists() {
        return Err(format!("{device} does not contain a NEXUS install payload"));
    }
    let version = fs::read_to_string(payload.join("VERSION"))
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(Media {
        parent_disk: whole_disk_of(&device),
        device,
        version,
        payload,
    })
}

fn wait_for(path: &str, max: Duration) -> Result<(), String> {
    let start = Instant::now();
    while start.elapsed() < max {
        if Path::new(path).exists() {
            return Ok(());
        }
        let _ = Command::new("mdev").arg("-s").status();
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(format!("{path} did not appear"))
}

fn write_file(path: &Path, content: &str, mode: u32) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("{}: {e}", path.display()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())
}

fn grub_cfg(version: &str) -> String {
    format!(
        r#"# NEXUS OS {version} boot configuration (A/B system slots).
# Boot state lives in grubenv; see documentation/updates.md.
set timeout=3
set default=0
insmod part_gpt
insmod fat
insmod loadenv
serial --unit=0 --speed=115200
terminal_input console serial
terminal_output console serial

load_env -f $prefix/grubenv
if [ -z "$nexus_slot" ]; then set nexus_slot=a; fi
set boot_slot=$nexus_slot
set trial=""
if [ -n "$nexus_try" ]; then
  if [ "$nexus_tries" = "3" ]; then
    set nexus_tries=2; set boot_slot=$nexus_try; set trial="nexus.trial=1"
  elif [ "$nexus_tries" = "2" ]; then
    set nexus_tries=1; set boot_slot=$nexus_try; set trial="nexus.trial=1"
  elif [ "$nexus_tries" = "1" ]; then
    set nexus_tries=0; set boot_slot=$nexus_try; set trial="nexus.trial=1"
  else
    set nexus_try=""
    set nexus_tries=0
  fi
  save_env -f $prefix/grubenv nexus_try nexus_tries
fi
if [ "$boot_slot" = "b" ]; then
  set root_uuid=$nexus_partuuid_b; set other_slot=a; set other_uuid=$nexus_partuuid_a
else
  set boot_slot=a
  set root_uuid=$nexus_partuuid_a; set other_slot=b; set other_uuid=$nexus_partuuid_b
fi
set nexus_args="rootfstype=squashfs ro rootwait nexus.mode=installed console=tty1 console=ttyS0,115200 panic=10 loglevel=4"

menuentry "NEXUS OS (slot $boot_slot)" {{
  linux /nexus/$boot_slot/bzImage root=PARTUUID=$root_uuid nexus.slot=$boot_slot $trial $nexus_args
}}
menuentry "NEXUS OS (other slot $other_slot)" {{
  linux /nexus/$other_slot/bzImage root=PARTUUID=$other_uuid nexus.slot=$other_slot $nexus_args
}}
"#
    )
}

/// Erase `disk` and install. Progress lines go to `say`.
pub fn install(
    media: &Media,
    disk: &Disk,
    a: &Answers,
    say: &mut dyn FnMut(&str),
) -> Result<Summary, String> {
    let dev = format!("/dev/{}", disk.name);
    log(&format!(
        "install to {dev} ({}), version {}",
        disk.describe(),
        media.version
    ));
    if Some(&disk.name) == media.parent_disk.as_ref() {
        return Err("refusing to install onto the installation media".into());
    }
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    if mounts.lines().any(|l| l.starts_with(&dev)) {
        return Err(format!("{dev} has mounted filesystems; unmount them first"));
    }

    say("Verifying installation payload");
    let o = Command::new("sha256sum")
        .args(["-c", "SHA256SUMS"])
        .current_dir(&media.payload)
        .output()
        .map_err(|e| e.to_string())?;
    if !o.status.success() {
        log(&String::from_utf8_lossy(&o.stdout));
        return Err(
            "installation media is corrupted (checksum mismatch) — rewrite the USB stick".into(),
        );
    }
    let image = media.payload.join("system.squashfs");
    let kernel = media.payload.join("bzImage");

    say(&format!("Partitioning {dev}"));
    let _ = Command::new("swapoff").arg("-a").status();
    exec("wipefs", &["-a", "-f", &dev])?;
    let layout = "label: gpt\n\
        size=1MiB, type=21686148-6449-6E6F-744E-656564454649, name=\"BIOSBOOT\"\n\
        size=128MiB, type=C12A7328-F81F-11D2-BA4B-00A0C93EC93B, name=\"NEXUS_ESP\"\n\
        size=1GiB, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"NEXUS_A\"\n\
        size=1GiB, type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"NEXUS_B\"\n\
        type=0FC63DAF-8483-4772-8E79-3D69D8477DE4, name=\"NEXUS_DATA\"\n";
    run_stdin(
        "sfdisk",
        &[
            "--wipe",
            "always",
            "--wipe-partitions",
            "always",
            "-q",
            &dev,
        ],
        layout,
    )?;
    let _ = Command::new("blockdev").args(["--rereadpt", &dev]).status();
    let p: Vec<String> = (1..=5).map(|n| disks::part_dev(&disk.name, n)).collect();
    for part in &p {
        wait_for(part, Duration::from_secs(15))?;
    }

    say("Creating filesystems");
    exec("mkfs.vfat", &["-F", "32", "-n", "NEXUS_ESP", &p[1]])?;
    exec("mkfs.ext4", &["-q", "-F", "-L", "NEXUS_DATA", &p[4]])?;

    say("Writing system image to slots A and B");
    for part in [&p[2], &p[3]] {
        exec(
            "dd",
            &[
                &format!("if={}", image.display()),
                &format!("of={part}"),
                "bs=1M",
                "conv=fsync",
            ],
        )?;
    }
    let partuuid = |d: &str| -> Result<String, String> {
        Ok(exec("blkid", &["-s", "PARTUUID", "-o", "value", d])?
            .trim()
            .to_string())
    };
    let (uuid_a, uuid_b) = (partuuid(&p[2])?, partuuid(&p[3])?);
    if uuid_a.is_empty() || uuid_b.is_empty() {
        return Err("cannot read partition UUIDs".into());
    }

    say("Installing boot loader (UEFI and BIOS)");
    fs::create_dir_all(ESP_MNT).map_err(|e| e.to_string())?;
    exec("mount", &["-t", "vfat", &p[1], ESP_MNT])?;
    let esp_result = (|| -> Result<(), String> {
        for slot in ["a", "b"] {
            let d = Path::new(ESP_MNT).join("nexus").join(slot);
            fs::create_dir_all(&d).map_err(|e| e.to_string())?;
            fs::copy(&kernel, d.join("bzImage")).map_err(|e| e.to_string())?;
        }
        exec(
            "grub-install",
            &[
                "--target=x86_64-efi",
                &format!("--efi-directory={ESP_MNT}"),
                &format!("--boot-directory={ESP_MNT}"),
                "--removable",
                "--no-nvram",
            ],
        )?;
        exec(
            "grub-install",
            &[
                "--target=i386-pc",
                &format!("--boot-directory={ESP_MNT}"),
                &dev,
            ],
        )?;
        write_file(
            &Path::new(ESP_MNT).join("grub/grub.cfg"),
            &grub_cfg(&media.version),
            0o644,
        )?;
        let env = format!("{ESP_MNT}/grub/grubenv");
        exec("grub-editenv", &[&env, "create"])?;
        exec(
            "grub-editenv",
            &[
                &env,
                "set",
                "nexus_slot=a",
                "nexus_try=",
                "nexus_tries=0",
                &format!("nexus_partuuid_a={uuid_a}"),
                &format!("nexus_partuuid_b={uuid_b}"),
            ],
        )?;
        Ok(())
    })();
    let _ = Command::new("sync").status();
    let _ = exec("umount", &[ESP_MNT]);
    esp_result?;

    say("Writing configuration and administrator account");
    fs::create_dir_all(DATA_MNT).map_err(|e| e.to_string())?;
    exec("mount", &["-t", "ext4", &p[4], DATA_MNT])?;
    let data_result = populate_data(media, disk, a);
    let _ = Command::new("sync").status();
    let _ = exec("umount", &[DATA_MNT]);
    let fingerprints = data_result?;
    let _ = Command::new("umount").arg(MEDIA_MNT).status();
    say("Installation complete");
    log("installation complete");

    let sample = nexus_core::resources::Sample::take();
    let ip = nexus_core::resources::network(None, &sample)
        .into_iter()
        .find_map(|n| n.ipv4.first().cloned());
    Ok(Summary {
        hostname: a.hostname.clone(),
        admin: a.admin.clone(),
        ip,
        fingerprints,
    })
}

fn populate_data(media: &Media, disk: &Disk, a: &Answers) -> Result<Vec<(String, String)>, String> {
    let d = Path::new(DATA_MNT);
    let etc = d.join("etc");
    for sub in [
        "etc/nexus",
        "etc/dropbear",
        "log",
        "home",
        "nexus/projects",
        "nexus/audit",
        "var/lib",
    ] {
        fs::create_dir_all(d.join(sub)).map_err(|e| e.to_string())?;
    }
    let factory = |f: &str| {
        fs::read_to_string(Path::new(FACTORY).join(f)).map_err(|e| format!("factory {f}: {e}"))
    };
    let days = nexus_core::time::now_unix() / 86_400;
    let name = &a.admin;

    let mut passwd = factory("passwd")?;
    passwd.push_str(&format!(
        "{name}:x:{ADMIN_UID}:{ADMIN_UID}:NEXUS Administrator:/home/{name}:/bin/sh\n"
    ));
    let mut group = String::new();
    for l in factory("group")?.lines() {
        if l.starts_with("wheel:") {
            let sep = if l.ends_with(':') { "" } else { "," };
            group.push_str(&format!("{l}{sep}{name}\n"));
        } else {
            group.push_str(l);
            group.push('\n');
        }
    }
    group.push_str(&format!("{name}:x:{ADMIN_UID}:\n"));
    let mut shadow = factory("shadow")?;
    shadow.push_str(&format!("{name}:{}:{days}:0:99999:7:::\n", a.password_hash));
    write_file(&etc.join("passwd"), &passwd, 0o644)?;
    write_file(&etc.join("group"), &group, 0o644)?;
    write_file(&etc.join("shadow"), &shadow, 0o600)?;
    write_file(&etc.join("hostname"), &format!("{}\n", a.hostname), 0o644)?;
    write_file(
        &etc.join("hosts"),
        &format!(
            "127.0.0.1\tlocalhost\n127.0.1.1\t{h} {h}.local\n::1\tlocalhost ip6-localhost\n",
            h = a.hostname
        ),
        0o644,
    )?;
    fs::copy(
        Path::new(FACTORY).join("nexus/network.conf"),
        etc.join("nexus/network.conf"),
    )
    .map_err(|e| e.to_string())?;
    write_file(
        &etc.join("nexus/ssh.conf"),
        "# Password logins are enabled for first-time key enrollment only.\n# 'nexus ssh finalize' switches to keys only.\nPASSWORD_AUTH=yes\nPORT=22\n",
        0o644,
    )?;
    let installed = serde_json::json!({
        "version": media.version,
        "installed_at": nexus_core::time::now_rfc3339(),
        "disk": { "name": disk.name, "model": disk.model, "size_bytes": disk.size, "transport": disk.transport },
        "firmware": if Path::new("/sys/firmware/efi").exists() { "uefi" } else { "bios" },
    });
    write_file(
        &etc.join("nexus/installed.json"),
        &serde_json::to_string_pretty(&installed).unwrap_or_default(),
        0o644,
    )?;

    let home = d.join("home").join(name);
    fs::create_dir_all(home.join(".ssh")).map_err(|e| e.to_string())?;
    for (p, mode) in [(home.clone(), 0o700), (home.join(".ssh"), 0o700)] {
        chown(&p, Some(ADMIN_UID), Some(ADMIN_UID)).map_err(|e| e.to_string())?;
        fs::set_permissions(&p, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
    }
    fs::set_permissions(d.join("etc/dropbear"), fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;
    fs::set_permissions(d.join("nexus/audit"), fs::Permissions::from_mode(0o700))
        .map_err(|e| e.to_string())?;

    let mut fps = Vec::new();
    for t in ["ed25519", "ecdsa"] {
        let key = d.join(format!("etc/dropbear/dropbear_{t}_host_key"));
        let k = key.to_string_lossy().into_owned();
        exec("dropbearkey", &["-t", t, "-f", &k])?;
        let out = exec("dropbearkey", &["-y", "-f", &k])?;
        if let Some(fp) = out.lines().find_map(|l| l.strip_prefix("Fingerprint: ")) {
            fps.push((t.to_string(), fp.trim().to_string()));
        }
    }
    let _ = fs::copy(crate::LOG, d.join("log/install.log"));
    Ok(fps)
}

#[cfg(test)]
mod tests {
    #[test]
    fn grub_cfg_is_balanced() {
        let c = super::grub_cfg("0.1.0");
        assert_eq!(c.matches('{').count(), c.matches('}').count());
        assert!(c.contains("root=PARTUUID=$root_uuid"));
        assert!(c.contains("save_env -f $prefix/grubenv nexus_try nexus_tries"));
    }
}
