//! nexus-install — the NEXUS OS installation wizard.
//!
//! Runs on the live installer system (tty1 / ttyS0). Interactive by default;
//! `--answers FILE` runs unattended from a TOML answers file (tests, fleet
//! installs). The selected disk is erased only after the operator types its
//! device name. Passwords are hashed (SHA-512-crypt) before they touch disk
//! and never appear in logs.

mod disks;
mod install;

use nexus_core::term;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

pub const LOG: &str = "/tmp/nexus-install.log";

/// Installer log. Callers must never pass secrets.
pub fn log(msg: &str) {
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(LOG)
    {
        let _ = writeln!(f, "{} {msg}", nexus_core::time::now_rfc3339());
    }
}

fn say(s: &str) {
    println!("{s}");
}

fn prompt(label: &str, default: Option<&str>) -> io::Result<String> {
    match default {
        Some(d) => print!("{label} [{d}]:\n> "),
        None => print!("{label}:\n> "),
    }
    io::stdout().flush()?;
    let mut s = String::new();
    if io::stdin().lock().read_line(&mut s)? == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "input closed"));
    }
    let s = s.trim().to_string();
    Ok(if s.is_empty() {
        default.unwrap_or("").to_string()
    } else {
        s
    })
}

pub fn valid_hostname(h: &str) -> bool {
    let b = h.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

const RESERVED_USERS: &[&str] = &[
    "root", "daemon", "bin", "sys", "sync", "mail", "www-data", "operator", "nobody", "sshd",
    "dropbear", "nexus", "admin-", "wheel", "users", "nogroup",
];

pub fn valid_username(u: &str) -> bool {
    let b = u.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_lowercase() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
        && !RESERVED_USERS.contains(&u)
        && !u.starts_with("nx-")
        && !u.starts_with("agent-")
}

pub fn password_problem(pw: &str, user: &str) -> Option<&'static str> {
    if pw.chars().count() < 8 {
        Some("Password must be at least 8 characters.")
    } else if pw.len() > 200 {
        Some("Password is too long (max 200 bytes).")
    } else if pw == user {
        Some("Password must differ from the username.")
    } else if pw.chars().any(|c| c.is_control()) {
        Some("Password contains control characters.")
    } else {
        None
    }
}

/// SHA-512-crypt hash with a random salt (supported by musl crypt(3),
/// which BusyBox login and Dropbear use).
pub fn hash_password(pw: &str) -> io::Result<String> {
    use sha_crypt::{Algorithm, Params, PasswordHasher, ShaCrypt};
    let mut salt = [0u8; 12];
    let mut f = fs::File::open("/dev/urandom")?;
    io::Read::read_exact(&mut f, &mut salt)?;
    let params = Params::new(100_000).map_err(|e| io::Error::other(e.to_string()))?;
    let hasher = ShaCrypt::new(Algorithm::Sha512Crypt, params);
    let h = hasher
        .hash_password_with_salt(pw.as_bytes(), &salt)
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(h.to_string())
}

pub struct Answers {
    pub hostname: String,
    pub admin: String,
    pub password_hash: String,
    pub disk: String,
}

fn parse_answers(path: &str) -> Result<Answers, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let v: toml::Table = toml::from_str(&text).map_err(|e| e.to_string())?;
    let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
    let hostname = get("hostname").ok_or("answers: hostname missing")?;
    let admin = get("admin").ok_or("answers: admin missing")?;
    let disk = get("disk").ok_or("answers: disk missing")?;
    if get("confirm_erase").as_deref() != Some(disk.as_str()) {
        return Err("answers: confirm_erase must repeat the disk name".into());
    }
    if !valid_hostname(&hostname) || !valid_username(&admin) {
        return Err("answers: invalid hostname or admin name".into());
    }
    let password_hash = match (get("password_hash"), get("password")) {
        (Some(h), _) if h.starts_with("$6$") => h,
        (_, Some(p)) => {
            if let Some(e) = password_problem(&p, &admin) {
                return Err(format!("answers: {e}"));
            }
            hash_password(&p).map_err(|e| e.to_string())?
        }
        _ => return Err("answers: password_hash ($6$...) or password required".into()),
    };
    Ok(Answers {
        hostname,
        admin,
        password_hash,
        disk,
    })
}

fn lock() -> Option<fs::File> {
    let _ = fs::create_dir_all("/run/nexus");
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open("/run/nexus/installer.lock")
        .ok()?;
    let r = unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&f),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    };
    (r == 0).then_some(f)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        say("usage: nexus-install [--console] [--answers FILE]");
        return;
    }
    let answers_path = args
        .iter()
        .position(|a| a == "--answers")
        .and_then(|i| args.get(i + 1).cloned());
    let r = match answers_path {
        Some(p) => unattended(&p),
        None => interactive(),
    };
    match r {
        Ok(()) => {}
        Err(e) => {
            log(&format!("installer stopped: {e}"));
            say(&format!("\nInstallation stopped: {e}"));
            say("No further changes were made. Details: /tmp/nexus-install.log");
            if args.iter().any(|a| a == "--console") {
                say("Press Enter to restart the installer, or type 'shell'.");
                let mut s = String::new();
                let _ = io::stdin().read_line(&mut s);
                if s.trim() == "shell" {
                    exec_shell();
                }
            }
            std::process::exit(1);
        }
    }
}

fn exec_shell() -> ! {
    use std::os::unix::process::CommandExt;
    let e = Command::new("/bin/sh").arg("-l").exec();
    eprintln!("cannot start shell: {e}");
    std::process::exit(1);
}

fn unattended(path: &str) -> Result<(), String> {
    let _lock = lock().ok_or("another installer instance is running")?;
    let a = parse_answers(path)?;
    log(&format!(
        "unattended install: hostname={} admin={} disk={}",
        a.hostname, a.admin, a.disk
    ));
    let media = install::find_media()?;
    let candidates = disks::eligible(media.parent_disk.as_deref());
    let disk = candidates
        .iter()
        .find(|d| d.name == a.disk)
        .ok_or_else(|| format!("disk {} is not eligible for installation", a.disk))?
        .clone();
    let summary = install::install(&media, &disk, &a, &mut |m| say(&format!(" * {m}")))?;
    print_summary(&summary);
    Ok(())
}

fn banner() {
    print!("{}", term::CLEAR);
    say("=================================================");
    say("               NEXUS OS INSTALLER");
    say("=================================================");
    say("");
}

fn interactive() -> Result<(), String> {
    let io_err = |e: io::Error| e.to_string();
    // 1. Welcome
    banner();
    say("Welcome to NEXUS.\n");
    say("This wizard installs NEXUS OS, a lightweight server operating system,");
    say("onto a disk of this computer. You will be asked for a computer name,");
    say("an administrator account and the installation disk.\n");
    let c = prompt(
        "Press Enter to begin, or type 'shell' or 'poweroff'",
        Some("begin"),
    )
    .map_err(io_err)?;
    match c.as_str() {
        "shell" => exec_shell(),
        "poweroff" => {
            let _ = Command::new("/sbin/poweroff").status();
            return Ok(());
        }
        _ => {}
    }
    // The wizard starts on every console; the first one used wins.
    let Some(_lock) = lock() else {
        say("\nThe installer is already running on another console.");
        say("Press Enter for a shell.");
        let mut s = String::new();
        let _ = io::stdin().read_line(&mut s);
        exec_shell();
    };

    // 2. Hardware check
    banner();
    say("HARDWARE CHECK\n");
    let hw = disks::hardware();
    say(&format!(" CPU:       {} ({} cores)", hw.cpu, hw.cores));
    say(&format!(" Memory:    {} MiB", hw.mem_mib));
    say(&format!(
        " Firmware:  {}",
        if hw.uefi { "UEFI" } else { "BIOS (legacy)" }
    ));
    let media = install::find_media();
    let parent = media.as_ref().ok().and_then(|m| m.parent_disk.clone());
    let candidates = disks::eligible(parent.as_deref());
    say(&format!(
        " Disks:     {} suitable for installation",
        candidates.len()
    ));
    if hw.mem_mib < 256 {
        return Err("at least 256 MiB of RAM is required".into());
    }
    if hw.mem_mib < 512 {
        say(
            "\n WARNING: less than 512 MiB of RAM; NEXUS will run, but leave little room for applications.",
        );
    }
    let media = media?;
    say(&format!(
        " Media:     NEXUS OS {} on {}",
        media.version, media.device
    ));
    if candidates.is_empty() {
        return Err(format!(
            "no disk of at least {} GiB found (the installation media itself is excluded)",
            disks::MIN_BYTES >> 30
        ));
    }
    say("\n Hardware check passed.");

    // 3. Network detection
    say("\nNETWORK DETECTION\n");
    let net = wait_for_network(Duration::from_secs(12));
    match &net {
        Some((iface, ip)) => say(&format!(" Connected: {iface} {ip}")),
        None => say(
            " No network address yet. You can continue; NEXUS uses DHCP on boot\n and can be configured later with 'nexus system network'.",
        ),
    }
    prompt("\nPress Enter to continue", Some("continue")).map_err(io_err)?;

    // 4-7. Identity
    banner();
    let hostname = loop {
        let h = prompt("Computer hostname", Some("nexus"))
            .map_err(io_err)?
            .to_lowercase();
        if valid_hostname(&h) {
            break h;
        }
        say(" Use 1-63 characters: a-z, 0-9 and '-' (not at the start or end).\n");
    };
    let admin = loop {
        let u = prompt("\nAdministrator username", Some("admin")).map_err(io_err)?;
        if valid_username(&u) {
            break u;
        }
        say(
            " Use lowercase letters, digits, '_' or '-', starting with a letter (max 32); system names are reserved.",
        );
    };
    let password_hash = loop {
        let p1 = term::read_hidden("\nAdministrator password:\n> ").map_err(io_err)?;
        if let Some(problem) = password_problem(&p1, &admin) {
            say(&format!(" {problem}"));
            continue;
        }
        let p2 = term::read_hidden("\nConfirm password:\n> ").map_err(io_err)?;
        if p1 != p2 {
            say(" The passwords do not match. Please try again.");
            continue;
        }
        say(" Hashing password...");
        break hash_password(&p1).map_err(io_err)?;
    };

    // 8-9. Disk
    let disk = loop {
        banner();
        say("INSTALLATION DISK\n");
        for (i, d) in candidates.iter().enumerate() {
            say(&format!(" [{}] {}", i + 1, d.describe()));
            if !d.partitions.is_empty() {
                say(&format!("      contains: {}", d.partitions.join(", ")));
            }
        }
        let c = prompt(
            "\nSelect the disk to install to",
            if candidates.len() == 1 {
                Some("1")
            } else {
                None
            },
        )
        .map_err(io_err)?;
        let Some(d) = c
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|i| candidates.get(i))
        else {
            say(" Please enter one of the listed numbers.");
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        say(&format!(
            "\nWARNING: The selected disk will be erased.\n\n /dev/{}  {}\n",
            d.name,
            d.describe()
        ));
        say("All partitions and data on this disk will be permanently destroyed.");
        let confirm = prompt(
            &format!(
                "Type the disk name ({}) to erase it and install, or 'back'",
                d.name
            ),
            None,
        )
        .map_err(io_err)?;
        if confirm == d.name {
            break d.clone();
        }
        if confirm != "back" {
            say(" Confirmation did not match; nothing was changed.");
            std::thread::sleep(Duration::from_secs(2));
        }
    };

    // 10. Install
    banner();
    say(&format!(
        "[INSTALL] NEXUS OS {} -> /dev/{}\n",
        media.version, disk.name
    ));
    let answers = Answers {
        hostname,
        admin,
        password_hash,
        disk: disk.name.clone(),
    };
    let summary = install::install(&media, &disk, &answers, &mut |m| say(&format!(" * {m}")))?;

    // 11. Done
    banner();
    print_summary(&summary);
    prompt(
        "\nRemove the installation media, then press Enter to reboot",
        Some("reboot"),
    )
    .map_err(io_err)?;
    let _ = Command::new("/sbin/reboot").status();
    Ok(())
}

fn wait_for_network(max: Duration) -> Option<(String, String)> {
    let start = Instant::now();
    loop {
        let sample = nexus_core::resources::Sample::take();
        for n in nexus_core::resources::network(None, &sample) {
            if let Some(ip) = n.ipv4.first() {
                return Some((n.name, ip.clone()));
            }
        }
        if start.elapsed() > max {
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn print_summary(s: &install::Summary) {
    say("NEXUS OS INSTALLED\n");
    say(&format!("Hostname: {}", s.hostname));
    say(&format!("Administrator: {}", s.admin));
    match &s.ip {
        Some(ip) => {
            say("Network: Connected");
            say(&format!("IP address: {ip}"));
        }
        None => say("Network: Not connected (DHCP will be retried at boot)"),
    }
    say("\nRemote access:\n");
    say(&format!("  ssh {}@{}.local", s.admin, s.hostname));
    if let Some(ip) = &s.ip {
        say(&format!(
            "  ssh {}@{}        (if .local names do not resolve)",
            s.admin,
            ip.split('/').next().unwrap_or(ip)
        ));
    }
    say("\nSSH host key fingerprints:");
    for (t, fp) in &s.fingerprints {
        say(&format!("  {t:<8} {fp}"));
    }
    say("\nWhen you connect the first time, SSH shows the server's fingerprint and asks");
    say("whether to trust it. Compare it with the line above: if it matches, answer");
    say("'yes'. If it does not match, do not continue — someone may be intercepting.");
    say("\nPassword logins over SSH are enabled only for first-time setup. Add your");
    say("SSH key (ssh-copy-id or 'nexus ssh add-key'), log in with it, then run");
    say("'nexus ssh finalize' to switch to key-only access.");
    say("\nTip: enable 'Restore on AC power loss' / 'Power on after power failure' in the");
    say("firmware setup so the server starts again by itself after a power cut.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert!(valid_hostname("pixel"));
        assert!(valid_hostname("my-server-01"));
        assert!(!valid_hostname("-bad"));
        assert!(!valid_hostname("Bad"));
        assert!(!valid_hostname("a.b"));
        assert!(valid_username("admin"));
        assert!(!valid_username("root"));
        assert!(!valid_username("nx-web"));
        assert!(!valid_username("9lives"));
        assert!(password_problem("short", "admin").is_some());
        assert!(password_problem("adminadmin", "adminadmin").is_some());
        assert!(password_problem("correct horse", "admin").is_none());
    }

    #[test]
    fn password_hash_format() {
        let h = hash_password("correct horse").unwrap();
        assert!(h.starts_with("$6$rounds=100000$"), "{h}");
        let parts: Vec<&str> = h.split('$').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[3].len(), 16);
        assert_ne!(
            h,
            hash_password("correct horse").unwrap(),
            "salt must be random"
        );
    }
}
