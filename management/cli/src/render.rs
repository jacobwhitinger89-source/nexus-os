//! Human-readable renderings. Machine output is the JSON returned by the
//! daemon or collected locally, printed verbatim with `--json`.

use nexus_core::resources::{Resources, human_bytes};
use nexus_core::schema::{
    DeploymentRecord, Diagnostic, Health, ProjectState, ProjectStatus, ProjectSummary, ServiceState,
};
use nexus_core::time::human_duration;
use serde_json::Value;
use std::fmt::Write;

pub const RULE: &str = "=================================================";
pub const LINE: &str = "-------------------------------------------------";

pub fn header(out: &mut String, title: &str) {
    let pad = RULE.len().saturating_sub(title.len()) / 2;
    let _ = writeln!(out, "{RULE}\n{}{title}\n{RULE}", " ".repeat(pad));
}

pub fn section(out: &mut String, title: &str) {
    let _ = writeln!(out, "\n {title}\n{LINE}");
}

pub fn rate(bps: Option<f64>) -> String {
    match bps {
        None => "measuring".into(),
        Some(b) if b >= 1024.0 * 1024.0 => format!("{:.1} MB/s", b / 1024.0 / 1024.0),
        Some(b) if b >= 1024.0 => format!("{:.1} KB/s", b / 1024.0),
        Some(b) => format!("{b:.0} B/s"),
    }
}

pub fn service_label(s: ServiceState, h: Health) -> &'static str {
    match (s, h) {
        (ServiceState::Running, Health::Unhealthy) => "UNHEALTHY",
        (ServiceState::Running, _) => "ACTIVE",
        (ServiceState::Starting, _) => "STARTING",
        (ServiceState::Waiting, _) => "WAITING",
        (ServiceState::Stopping, _) => "STOPPING",
        (ServiceState::Backoff, _) => "RESTARTING",
        (ServiceState::Failed, _) => "FAILED",
        (ServiceState::Exited, _) => "COMPLETED",
        (ServiceState::Stopped, _) => "INACTIVE",
    }
}

/// Primary data storage (the /data mount, else /).
pub fn primary_storage(r: &Resources) -> Option<&nexus_core::resources::Storage> {
    r.storage
        .iter()
        .find(|s| s.mount == "/data" && s.filesystem != "tmpfs")
        .or_else(|| r.storage.iter().find(|s| s.mount == "/"))
        .or_else(|| r.storage.first())
}

pub fn resources_brief(out: &mut String, r: &Resources) {
    section(out, "SYSTEM RESOURCES");
    let _ = writeln!(
        out,
        " CPU:       {:.0}% USED ({} cores, load {:.2})",
        r.cpu.usage_percent, r.cpu.cores, r.cpu.load_average[0]
    );
    let _ = writeln!(
        out,
        " RAM:       {:.0}% USED ({} of {})",
        r.memory.used_percent,
        human_bytes(r.memory.used_bytes),
        human_bytes(r.memory.total_bytes)
    );
    match primary_storage(r) {
        Some(s) => {
            let _ = writeln!(
                out,
                " DISK:      {:.0}% USED ({} free on {})",
                s.used_percent,
                human_bytes(s.available_bytes),
                s.mount
            );
        }
        None => {
            let _ = writeln!(out, " DISK:      NOT DETECTED");
        }
    }
    let up: Vec<_> = r.network.iter().filter(|n| n.state == "up").collect();
    if up.is_empty() {
        let _ = writeln!(out, "\n NETWORK:   NO LINK");
    } else {
        for n in up {
            let ip = n.ipv4.first().map(|s| s.as_str()).unwrap_or("no IPv4");
            let _ = writeln!(out, "\n NETWORK:   {} {}", n.name, ip);
            let _ = writeln!(
                out,
                " RX: {}\n TX: {}",
                rate(n.rx_bytes_per_sec),
                rate(n.tx_bytes_per_sec)
            );
        }
    }
    let gpu = if r.gpus.is_empty() {
        "NOT DETECTED".to_string()
    } else {
        r.gpus
            .iter()
            .map(|g| match g.utilization_percent {
                Some(u) => format!("{} {:.0}% USED", g.vendor, u),
                None => format!("{} (metrics NOT SUPPORTED)", g.vendor),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(out, "\n GPU: {gpu}");
}

pub fn resources_full(r: &Resources) -> String {
    let mut o = String::new();
    header(&mut o, "NEXUS OS — RESOURCES");
    let _ = writeln!(
        o,
        " HOSTNAME: {}\n OS:       NEXUS OS {}\n KERNEL:   {}\n UPTIME:   {}",
        r.hostname,
        r.os_version,
        r.kernel,
        human_duration(r.uptime_seconds)
    );
    section(&mut o, "CPU");
    let _ = writeln!(
        o,
        " Model:      {}",
        r.cpu.model.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(o, " Cores:      {}", r.cpu.cores);
    let _ = writeln!(
        o,
        " Usage:      {:.1}%  (available {:.1}%)",
        r.cpu.usage_percent, r.cpu.available_percent
    );
    let _ = writeln!(
        o,
        " Load avg:   {:.2} {:.2} {:.2}",
        r.cpu.load_average[0], r.cpu.load_average[1], r.cpu.load_average[2]
    );
    let per: Vec<String> = r
        .cpu
        .per_core_percent
        .iter()
        .enumerate()
        .map(|(i, p)| format!("cpu{i} {p:.0}%"))
        .collect();
    for chunk in per.chunks(4) {
        let _ = writeln!(o, "   {}", chunk.join("   "));
    }
    section(&mut o, "MEMORY");
    let m = &r.memory;
    let _ = writeln!(o, " Total:      {}", human_bytes(m.total_bytes));
    let _ = writeln!(
        o,
        " Used:       {} ({:.1}%)",
        human_bytes(m.used_bytes),
        m.used_percent
    );
    let _ = writeln!(o, " Available:  {}", human_bytes(m.available_bytes));
    let _ = writeln!(o, " Cached:     {}", human_bytes(m.cached_bytes));
    if m.swap.configured {
        let _ = writeln!(
            o,
            " Swap:       {} of {}",
            human_bytes(m.swap.used_bytes),
            human_bytes(m.swap.total_bytes)
        );
    } else {
        let _ = writeln!(o, " Swap:       not configured");
    }
    section(&mut o, "STORAGE");
    for s in &r.storage {
        let _ = writeln!(
            o,
            " {:<12} {:<6} {:>9} total {:>9} used {:>9} free ({:.0}%){}",
            s.mount,
            s.filesystem,
            human_bytes(s.total_bytes),
            human_bytes(s.used_bytes),
            human_bytes(s.available_bytes),
            s.used_percent,
            if s.read_only { " ro" } else { "" }
        );
    }
    for d in &r.disks {
        let _ = writeln!(
            o,
            " {:<12} {:>9}  health {}{}{}",
            d.device,
            human_bytes(d.size_bytes),
            d.health,
            d.temperature_celsius
                .map(|t| format!(", {t}°C"))
                .unwrap_or_default(),
            d.model
                .as_ref()
                .map(|m| format!("  {m}"))
                .unwrap_or_default()
        );
    }
    section(&mut o, "NETWORK");
    if r.network.is_empty() {
        let _ = writeln!(o, " NOT DETECTED");
    }
    for n in &r.network {
        let _ = writeln!(
            o,
            " {:<8} {:<5} {}{}",
            n.name,
            n.state.to_uppercase(),
            n.mac.as_deref().unwrap_or(""),
            n.speed_mbps
                .map(|s| format!("  {s} Mb/s"))
                .unwrap_or_default()
        );
        for a in n.ipv4.iter().chain(n.ipv6.iter()) {
            let _ = writeln!(o, "          {a}");
        }
        let _ = writeln!(
            o,
            "          RX {}  TX {}  (total {} / {})",
            rate(n.rx_bytes_per_sec),
            rate(n.tx_bytes_per_sec),
            human_bytes(n.rx_bytes_total),
            human_bytes(n.tx_bytes_total)
        );
    }
    let _ = writeln!(
        o,
        " Connections: {} established, {} listening",
        r.connections.tcp_established, r.connections.tcp_listening
    );
    section(&mut o, "GPU");
    if r.gpus.is_empty() {
        let _ = writeln!(o, " NOT DETECTED");
    }
    for g in &r.gpus {
        let _ = writeln!(
            o,
            " {} {} [{}:{}] driver {}",
            g.pci_address,
            g.vendor,
            g.vendor_id,
            g.device_id,
            g.driver.as_deref().unwrap_or("none")
        );
        match g.utilization_percent {
            Some(u) => {
                let _ = writeln!(
                    o,
                    "   utilisation {u:.0}%, memory {} / {}",
                    g.memory_used_bytes.map(human_bytes).unwrap_or("?".into()),
                    g.memory_total_bytes.map(human_bytes).unwrap_or("?".into())
                );
            }
            None => {
                let _ = writeln!(o, "   utilisation: NOT SUPPORTED   memory: NOT SUPPORTED");
            }
        }
    }
    o
}

pub fn projects_table(list: &[ProjectSummary]) -> String {
    let mut o = String::new();
    if list.is_empty() {
        o.push_str(" No projects. Create one with: nexus project create <name>\n");
        return o;
    }
    let _ = writeln!(
        o,
        " {:<28} {:<13} {:>8}  RELEASE",
        "PROJECT", "STATUS", "SERVICES"
    );
    for p in list {
        let _ = writeln!(
            o,
            " {:<28} {:<13} {:>3}/{:<4}  {}",
            p.name,
            p.status.label(),
            p.services_running,
            p.services_total,
            p.release.as_deref().unwrap_or("-")
        );
    }
    o
}

pub fn diagnostic(o: &mut String, d: &Diagnostic, with_logs: bool) {
    let _ = writeln!(
        o,
        " [{:?}] {}{}: {}",
        d.severity,
        d.code,
        d.service
            .as_ref()
            .map(|s| format!(" ({s})"))
            .unwrap_or_default(),
        d.message
    );
    for s in &d.suggested_actions {
        let _ = writeln!(o, "   - {s}");
    }
    if !d.recovery_actions.is_empty() {
        let acts: Vec<&str> = d
            .recovery_actions
            .iter()
            .map(|a| a.command.as_str())
            .collect();
        let _ = writeln!(o, "   recovery: {}", acts.join("  |  "));
    }
    if with_logs && !d.logs.is_empty() {
        let _ = writeln!(o, "   recent log:");
        for l in d
            .logs
            .iter()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            let _ = writeln!(o, "     {l}");
        }
    }
}

pub fn project_status(s: &ProjectStatus) -> String {
    let mut o = String::new();
    header(&mut o, &format!("PROJECT: {}", s.project.to_uppercase()));
    let _ = writeln!(o, "\n STATUS: {}", s.status.label());
    if let Some(r) = &s.release {
        let _ = writeln!(
            o,
            " RELEASE: {}{}",
            r.id,
            if r.deployed_by.is_empty() {
                String::new()
            } else {
                format!(" (by {})", r.deployed_by)
            }
        );
    }
    section(&mut o, "SERVICES");
    if s.services.is_empty() {
        let _ = writeln!(o, " (none — deploy a release with a nexus.toml)");
    }
    for sv in &s.services {
        let mut extra = String::new();
        if let Some(pid) = sv.pid {
            let _ = write!(extra, " pid {pid}");
        }
        if let Some(u) = sv.uptime_seconds {
            let _ = write!(extra, " up {}", human_duration(u));
        }
        if sv.restarts > 0 {
            let _ = write!(extra, " restarts {}", sv.restarts);
        }
        if let Some(e) = &sv.last_exit
            && sv.pid.is_none()
        {
            match (e.code, e.signal) {
                (Some(c), _) => {
                    let _ = write!(extra, " last exit {c}");
                }
                (_, Some(sig)) => {
                    let _ = write!(extra, " last signal {sig}");
                }
                _ => {}
            }
        }
        let _ = writeln!(
            o,
            " {:<22} {:<10}{}",
            sv.name.to_uppercase(),
            service_label(sv.state, sv.health),
            extra
        );
    }
    section(&mut o, "APPLICATION METRICS");
    match &s.metrics {
        Some(Value::Object(m)) => {
            for (svc, v) in m {
                let inner = v.get("metrics").unwrap_or(v);
                if let Value::Object(kv) = inner {
                    for (k, val) in kv {
                        let label = if m.len() > 1 {
                            format!("{svc}.{k}")
                        } else {
                            k.clone()
                        };
                        let _ =
                            writeln!(o, " {:<22} {}", label.to_uppercase().replace('_', " "), val);
                    }
                }
            }
        }
        _ => {
            let _ = writeln!(
                o,
                " NOT PUBLISHED (applications report metrics via $NEXUS_METRICS_FILE)"
            );
        }
    }
    section(&mut o, "RESOURCE USAGE");
    let r = &s.resources;
    let _ = writeln!(
        o,
        " {:<22} {}",
        "CPU",
        r.cpu_percent
            .map(|c| format!("{c:.1}%"))
            .unwrap_or_else(|| "NOT AVAILABLE".into())
    );
    let _ = writeln!(
        o,
        " {:<22} {}{}",
        "MEMORY",
        r.memory_bytes
            .map(human_bytes)
            .unwrap_or_else(|| "NOT AVAILABLE".into()),
        r.memory_limit_bytes
            .map(|l| format!(" of {}", human_bytes(l)))
            .unwrap_or_default()
    );
    if let Some(p) = r.pids {
        let _ = writeln!(o, " {:<22} {p}", "TASKS");
    }
    if let Some(f) = &s.failure
        && s.status != ProjectState::Active
    {
        section(&mut o, "CURRENT PROBLEM");
        diagnostic(&mut o, f, false);
    }
    o
}

pub fn doctor(v: &Value) -> String {
    let mut o = String::new();
    header(
        &mut o,
        &format!(
            "DIAGNOSTICS: {}",
            v["project"].as_str().unwrap_or("?").to_uppercase()
        ),
    );
    let healthy = v["healthy"].as_bool().unwrap_or(false);
    let _ = writeln!(
        o,
        "\n RESULT: {}",
        if healthy {
            "NO PROBLEMS FOUND"
        } else {
            "PROBLEMS FOUND"
        }
    );
    section(&mut o, "CHECKS");
    for c in v["checks"].as_array().into_iter().flatten() {
        let mark = match c["status"].as_str() {
            Some("pass") => "PASS",
            Some("fail") => "FAIL",
            _ => "WARN",
        };
        let _ = writeln!(
            o,
            " [{mark}] {:<18} {}",
            c["check"].as_str().unwrap_or(""),
            c["message"].as_str().unwrap_or("")
        );
    }
    let diags: Vec<Diagnostic> =
        serde_json::from_value(v["diagnostics"].clone()).unwrap_or_default();
    if !diags.is_empty() {
        section(&mut o, "FINDINGS");
        for d in &diags {
            diagnostic(&mut o, d, true);
        }
    }
    o
}

pub fn history(list: &[DeploymentRecord]) -> String {
    let mut o = String::new();
    if list.is_empty() {
        o.push_str(" No deployments yet.\n");
    }
    for r in list {
        let _ = writeln!(
            o,
            " {:<26} {:<12} {:<20} {}{}",
            r.id,
            format!("{:?}", r.status).to_uppercase(),
            r.started_at,
            r.deployed_by,
            r.error
                .as_ref()
                .map(|e| format!("  {}: {}", e.code, e.message))
                .unwrap_or_default()
        );
    }
    o
}
