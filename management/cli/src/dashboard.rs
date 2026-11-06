//! Interactive terminal dashboard (shown on SSH login for administrators).
//!
//! Plain ANSI output, numeric menu selection, refresh every 3 s while
//! idle. Resource rates are computed from the previous refresh, so the
//! dashboard costs one /proc read per refresh and nothing in between.

use crate::render::{self, LINE};
use nexus_core::ipc::Request;
use nexus_core::resources::{self, Resources, Sample};
use nexus_core::schema::{DeploymentRecord, ProjectStatus, ProjectSummary};
use nexus_core::term::{self, BOLD, CLEAR, DIM, GREEN, RED, RESET, YELLOW};
use nexus_core::time::human_duration;
use serde_json::Value;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::time::Duration;

const REFRESH_MS: i32 = 3000;

struct Ui {
    sample: Option<Sample>,
}

fn call(req: Request) -> Result<Value, String> {
    match nexus_core::ipc::call(&req, None, |_, _| {}) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("{} ({})", e.message, e.code)),
        Err(e) => Err(e.to_string()),
    }
}

fn paint(s: &str) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(CLEAR.as_bytes());
    let _ = out.write_all(s.replace('\n', "\r\n").as_bytes());
    let _ = out.flush();
}

fn colour_state(label: &str) -> String {
    let c = match label {
        "ACTIVE" | "ONLINE" | "COMPLETED" | "RUNNING" => GREEN,
        "FAILED" | "UNHEALTHY" | "STOPPED" => RED,
        "INACTIVE" | "NOT DEPLOYED" => DIM,
        _ => YELLOW,
    };
    format!("{c}{label}{RESET}")
}

/// Read a menu choice: digits + Enter, a single key when unambiguous,
/// or None on refresh timeout. `max` is the highest valid number.
fn choose(prompt: &str, screen: &str, max: usize) -> Option<usize> {
    let mut buf = String::new();
    loop {
        paint(&format!("{screen}\n{prompt}{buf}"));
        match term::read_key(REFRESH_MS) {
            None => {
                if buf.is_empty() {
                    return None;
                }
            }
            Some(b'\r') | Some(b'\n') => {
                if let Ok(n) = buf.parse::<usize>() {
                    return Some(n);
                }
                buf.clear();
            }
            Some(3) | Some(b'q') => return Some(0),
            Some(127) | Some(8) => {
                buf.pop();
            }
            Some(b'r') => return None,
            Some(c) if c.is_ascii_digit() => {
                buf.push(c as char);
                // Answer immediately when no longer number could match.
                let n: usize = buf.parse().unwrap_or(0);
                if n * 10 > max {
                    return Some(n);
                }
            }
            _ => {}
        }
    }
}

fn wait_key(screen: &str) {
    paint(&format!("{screen}\n{DIM}Press any key to go back{RESET}"));
    loop {
        if term::read_key(60_000).is_some() {
            return;
        }
    }
}

impl Ui {
    fn resources(&mut self) -> Resources {
        let (r, s) = resources::collect(self.sample.as_ref(), Duration::from_millis(200), false);
        self.sample = Some(s);
        r
    }
}

pub fn run() -> io::Result<()> {
    if !term::is_tty(0) || !term::is_tty(1) {
        return Err(io::Error::other(
            "the dashboard needs an interactive terminal; use 'nexus status' instead",
        ));
    }
    let _raw =
        term::RawMode::enable(0).ok_or_else(|| io::Error::other("cannot configure terminal"))?;
    let mut ui = Ui { sample: None };
    loop {
        let r = ui.resources();
        let projects = call(Request::Projects).map(|v| {
            serde_json::from_value::<Vec<ProjectSummary>>(v["projects"].clone()).unwrap_or_default()
        });
        let mut s = String::new();
        render::header(&mut s, "NEXUS OS");
        let online = projects.is_ok();
        let _ = writeln!(
            s,
            "\n HOSTNAME: {BOLD}{}{RESET}\n STATUS: {}\n UPTIME: {}",
            r.hostname,
            colour_state(if online { "ONLINE" } else { "DEGRADED" }),
            human_duration(r.uptime_seconds)
        );
        render::resources_brief(&mut s, &r);
        render::section(&mut s, "PROJECTS");
        let list = projects.as_ref().cloned().unwrap_or_default();
        match &projects {
            Err(e) => {
                let _ = writeln!(s, " {RED}supervisor not reachable:{RESET} {e}");
            }
            Ok(l) if l.is_empty() => {
                let _ = writeln!(
                    s,
                    " No projects yet. Create one: nexus project create <name>"
                );
            }
            Ok(l) => {
                for (i, p) in l.iter().enumerate() {
                    let _ = writeln!(
                        s,
                        " [{}] {:<24} {}",
                        i + 1,
                        p.name,
                        colour_state(p.status.label())
                    );
                }
            }
        }
        let n = list.len();
        render::section(&mut s, "SYSTEM");
        let items = [
            "System information",
            "Services",
            "Logs",
            "Security",
            "Settings",
        ];
        for (i, it) in items.iter().enumerate() {
            let _ = writeln!(s, " [{}] {it}", n + 1 + i);
        }
        let _ = writeln!(s, "\n [0] Exit");
        match choose("\nSelect an option: ", &s, n + items.len()) {
            None => continue,
            Some(0) => break,
            Some(k) if k <= n => project_screen(&mut ui, &list[k - 1].name),
            Some(k) if k == n + 1 => wait_key(&render::resources_full(&ui.resources())),
            Some(k) if k == n + 2 => wait_key(&services_screen()),
            Some(k) if k == n + 3 => wait_key(&system_logs()),
            Some(k) if k == n + 4 => wait_key(&security_screen()),
            Some(k) if k == n + 5 => settings_screen(),
            Some(_) => {}
        }
    }
    paint("");
    Ok(())
}

fn project_screen(_ui: &mut Ui, name: &str) {
    let mut message = String::new();
    loop {
        let st = call(Request::ProjectStatus {
            project: name.into(),
        })
        .and_then(|v| serde_json::from_value::<ProjectStatus>(v).map_err(|e| e.to_string()));
        let mut s = match &st {
            Ok(st) => render::project_status(st),
            Err(e) => format!("{RED}{e}{RESET}\n"),
        };
        render::section(&mut s, "MANAGEMENT");
        s.push_str(" [1] View logs\n [2] View errors\n [3] Restart project\n [4] Stop project\n [5] Start project\n [6] Deployment history\n [7] Developer diagnostics\n\n [0] Back\n");
        if !message.is_empty() {
            let _ = writeln!(s, "\n {YELLOW}{message}{RESET}");
        }
        let rows = term::size().1 as usize;
        let choice = match choose("\nSelect an option: ", &s, 7) {
            None => continue,
            Some(0) => return,
            Some(k) => k,
        };
        message = match choice {
            1 | 2 => {
                let errors = choice == 2;
                match call(Request::Logs {
                    project: name.into(),
                    service: None,
                    errors,
                    lines: rows.saturating_sub(6).max(10),
                }) {
                    Ok(v) => {
                        let mut o = String::new();
                        render::header(
                            &mut o,
                            &format!(
                                "{} {}",
                                name.to_uppercase(),
                                if errors { "ERRORS" } else { "LOGS" }
                            ),
                        );
                        for l in v["lines"].as_array().into_iter().flatten() {
                            let _ = writeln!(o, "{}", l.as_str().unwrap_or(""));
                        }
                        wait_key(&o);
                        String::new()
                    }
                    Err(e) => e,
                }
            }
            k @ 3..=5 => {
                let req = match k {
                    3 => Request::Restart {
                        project: name.into(),
                        service: None,
                    },
                    4 => Request::Stop {
                        project: name.into(),
                        service: None,
                    },
                    _ => Request::Start {
                        project: name.into(),
                        service: None,
                    },
                };
                match call(req) {
                    Ok(_) => format!("{} requested", ["restart", "stop", "start"][k - 3]),
                    Err(e) => e,
                }
            }
            6 => match call(Request::History {
                project: name.into(),
            }) {
                Ok(v) => {
                    let list: Vec<DeploymentRecord> =
                        serde_json::from_value(v["deployments"].clone()).unwrap_or_default();
                    let mut o = String::new();
                    render::header(&mut o, &format!("{} DEPLOYMENTS", name.to_uppercase()));
                    o.push_str(&render::history(&list));
                    wait_key(&o);
                    String::new()
                }
                Err(e) => e,
            },
            7 => match call(Request::Doctor {
                project: name.into(),
            }) {
                Ok(v) => {
                    wait_key(&render::doctor(&v));
                    String::new()
                }
                Err(e) => e,
            },
            _ => String::new(),
        };
    }
}

fn pid_of(name: &str) -> Option<u32> {
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        if comm.trim() == name {
            return Some(pid);
        }
    }
    None
}

fn services_screen() -> String {
    let mut o = String::new();
    render::header(&mut o, "SYSTEM SERVICES");
    let mode = std::fs::read_to_string("/run/nexus/mode").unwrap_or_default();
    let installed = mode.trim() == "installed";
    let svcs: [(&str, &str, bool); 9] = [
        ("nexusd", "NEXUS project supervisor", true),
        ("dropbear", "SSH server", installed),
        ("mdnsd", "mDNS (name.local)", installed),
        ("udhcpc", "DHCP client", false),
        ("ntpd", "time synchronisation", false),
        ("syslogd", "system log", true),
        ("klogd", "kernel log", true),
        ("watchdog", "hardware watchdog", false),
        ("acpid", "power button", false),
    ];
    let _ = writeln!(o);
    for (name, desc, required) in svcs {
        let st = match pid_of(name) {
            Some(pid) => format!("{GREEN}RUNNING{RESET}  pid {pid}"),
            None if required => format!("{RED}STOPPED{RESET}"),
            None => format!("{DIM}NOT RUNNING{RESET}"),
        };
        let _ = writeln!(o, " {:<10} {:<28} {st}", name, desc);
    }
    let _ = writeln!(
        o,
        "\n System daemons are supervised by init and restart automatically."
    );
    o
}

fn system_logs() -> String {
    let mut o = String::new();
    render::header(&mut o, "SYSTEM LOG");
    let rows = term::size().1 as usize;
    match std::fs::read_to_string("/var/log/messages") {
        Ok(t) => {
            let lines: Vec<&str> = t.lines().collect();
            for l in &lines[lines.len().saturating_sub(rows.saturating_sub(5))..] {
                let _ = writeln!(o, "{l}");
            }
        }
        Err(e) => {
            let _ = writeln!(
                o,
                " cannot read /var/log/messages: {e}\n (use: sudo tail /var/log/messages)"
            );
        }
    }
    o
}

fn security_screen() -> String {
    let mut o = String::new();
    render::header(&mut o, "SECURITY");
    match call(Request::SshStatus) {
        Ok(v) => {
            let pw = v["password_auth"] == true;
            let _ = writeln!(
                o,
                "\n SSH password logins: {}",
                if pw {
                    format!("{YELLOW}ENABLED{RESET} (run: nexus ssh finalize)")
                } else {
                    format!("{GREEN}DISABLED{RESET} (keys only)")
                }
            );
            for k in v["host_keys"].as_array().into_iter().flatten() {
                let _ = writeln!(
                    o,
                    " Host key {:<8} {}",
                    k["type"].as_str().unwrap_or(""),
                    k["fingerprint"].as_str().unwrap_or("")
                );
            }
        }
        Err(e) => {
            let _ = writeln!(o, " SSH status unavailable: {e}");
        }
    }
    let keys = std::env::var("HOME")
        .ok()
        .and_then(|h| std::fs::read_to_string(format!("{h}/.ssh/authorized_keys")).ok())
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .count()
        })
        .unwrap_or(0);
    let _ = writeln!(o, " Your authorized keys: {keys}");
    let _ = writeln!(
        o,
        " Firewall: inbound default deny; SSH rate limited; project ports opened by nexusd"
    );
    render::section(&mut o, "RECENT AUTHENTICATION FAILURES");
    let failures: Vec<String> = std::fs::read_to_string("/var/log/messages")
        .unwrap_or_default()
        .lines()
        .filter(|l| {
            l.contains("Bad password")
                || l.contains("nonexistent user")
                || l.contains("Bad PAM")
                || l.contains("ssh-ratelimit")
                || l.contains("Max auth tries")
        })
        .map(String::from)
        .collect();
    if failures.is_empty() {
        let _ = writeln!(o, " none recorded");
    }
    for l in failures.iter().rev().take(8) {
        let _ = writeln!(o, " {l}");
    }
    render::section(&mut o, "AGENTS");
    match call(Request::AgentList) {
        Ok(v) => {
            let list = v["agents"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                let _ = writeln!(
                    o,
                    " none (add one: nexus agent add <name> --key '<public key>')"
                );
            }
            for a in list {
                let _ = writeln!(
                    o,
                    " {:<22} {}",
                    a["user"].as_str().unwrap_or(""),
                    a["grants"]
                );
            }
        }
        Err(e) => {
            let _ = writeln!(o, " {e}");
        }
    }
    render::section(&mut o, "AUDIT TRAIL (latest)");
    if let Ok(v) = call(Request::Audit { lines: 8 }) {
        for e in v["entries"].as_array().into_iter().flatten() {
            let _ = writeln!(
                o,
                " {} {} {} {} {}",
                e["ts"].as_str().unwrap_or(""),
                e["user"].as_str().unwrap_or(""),
                e["action"].as_str().unwrap_or(""),
                e["project"].as_str().unwrap_or("-"),
                e["result"].as_str().unwrap_or("")
            );
        }
    }
    o
}

fn settings_screen() {
    let mut o = String::new();
    render::header(&mut o, "SETTINGS");
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|_| "(not readable)\n".into());
    let _ = writeln!(o, "\n Hostname: {}", resources::hostname());
    render::section(&mut o, "NETWORK (/data/etc/nexus/network.conf)");
    for l in read("/data/etc/nexus/network.conf")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let _ = writeln!(o, " {l}");
    }
    render::section(&mut o, "CHANGE SETTINGS");
    let _ = writeln!(o, " sudo nexus system hostname <name>");
    let _ = writeln!(o, " sudo nexus system network dhcp");
    let _ = writeln!(
        o,
        " sudo nexus system network static --address 192.168.1.50/24 --gateway 192.168.1.1"
    );
    let _ = writeln!(
        o,
        " nexus ssh finalize          (disable SSH passwords after key login)"
    );
    let _ = writeln!(o, " sudo nexus system update <bundle.tar>");
    let _ = writeln!(o, "{LINE}");
    wait_key(&o);
}
