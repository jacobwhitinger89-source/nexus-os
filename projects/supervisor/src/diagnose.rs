//! Failure analysis: turns raw facts (exit status, cgroup counters, logs,
//! listening sockets, disk space) into structured `Diagnostic`s.
//!
//! The engine only observes. It never edits application code and never runs
//! repair commands; it lists recovery actions for an operator or authorised
//! agent to choose from.

use crate::{cgroup, logs};
use nexus_core::manifest::Service;
use nexus_core::schema::{Diagnostic, ExitInfo, RecoveryAction, Severity, codes};
use nexus_core::{resources, time};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;

const LOG_LINES: usize = 20;

fn action(action: &str, command: String, permission: &str) -> RecoveryAction {
    RecoveryAction {
        action: action.into(),
        command,
        permission: permission.into(),
    }
}

fn standard_actions(project: &str, service: Option<&str>) -> Vec<RecoveryAction> {
    let svc = service
        .map(|s| format!(" --service {s}"))
        .unwrap_or_default();
    vec![
        action("view_logs", format!("nexus logs {project}{svc}"), "logs"),
        action(
            "restart",
            format!("nexus restart {project}{svc}"),
            "restart",
        ),
        action("rollback", format!("nexus rollback {project}"), "deploy"),
    ]
}

fn base(
    code: &str,
    severity: Severity,
    project: &str,
    service: Option<&str>,
    message: String,
) -> Diagnostic {
    Diagnostic {
        code: code.into(),
        severity,
        project: project.into(),
        service: service.map(String::from),
        message,
        timestamp: time::now_rfc3339(),
        details: Value::Null,
        logs: service
            .map(|s| logs::tail_service(project, s, LOG_LINES))
            .unwrap_or_default(),
        suggested_actions: Vec::new(),
        recovery_actions: standard_actions(project, service),
    }
}

fn logs_mention(lines: &[String], needles: &[&str]) -> bool {
    lines.iter().any(|l| {
        let l = l.to_lowercase();
        needles.iter().any(|n| l.contains(n))
    })
}

/// Who listens on `port`, if it is not `own_pids`.
fn port_owner(port: u16) -> Option<(u32, String)> {
    resources::listening_tcp_ports()
        .into_iter()
        .find(|(p, _)| *p == port)
        .and_then(|(_, inode)| resources::socket_owner(inode))
}

pub fn port_conflict_details(spec: &Service) -> Option<(u16, Value)> {
    for p in &spec.ports {
        if let Some((pid, comm)) = port_owner(p.port()) {
            let project = owning_project(pid);
            return Some((
                p.port(),
                json!({ "port": p.port(), "owner_pid": pid, "owner_command": comm, "owner_project": project }),
            ));
        }
    }
    None
}

/// Map a pid to a NEXUS project through its cgroup path.
fn owning_project(pid: u32) -> Option<String> {
    let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let path = cg.lines().find_map(|l| l.strip_prefix("0::"))?;
    let rest = path.strip_prefix("/nexus/")?;
    rest.split('/').next().map(String::from)
}

/// Analyse an unexpected exit.
pub fn exit(
    project: &str,
    name: &str,
    spec: &Service,
    info: &ExitInfo,
    ran_for: Duration,
    cgroups: bool,
    oom_before: u64,
) -> Diagnostic {
    let svc_cg = cgroup::service_dir(project, name);
    let proj_cg = cgroup::project_dir(project);
    let recent = logs::tail_service(project, name, LOG_LINES);
    let exit_json = json!({ "exit_code": info.code, "signal": info.signal, "ran_for_seconds": ran_for.as_secs() });

    // Resource exhaustion first: it explains most signal deaths.
    let oom_now = if cgroups {
        cgroup::oom_kills(&svc_cg).unwrap_or(0)
    } else {
        0
    };
    if oom_now > oom_before
        || (info.signal == Some(libc::SIGKILL) && logs_mention(&recent, &["out of memory"]))
    {
        let limit = cgroups.then(|| cgroup::memory_max(&proj_cg)).flatten();
        let mut d = base(
            codes::OUT_OF_MEMORY,
            Severity::Error,
            project,
            Some(name),
            format!(
                "Service {name} was killed because the project exceeded its memory limit{}.",
                limit
                    .map(|l| format!(" ({})", resources::human_bytes(l)))
                    .unwrap_or_default()
            ),
        );
        d.details = json!({ "exit": exit_json, "memory_limit_bytes": limit, "oom_kills": oom_now });
        d.suggested_actions = vec![
            "Reduce the service's memory usage or raise the project limit.".into(),
            format!("Administrator: nexus project limits {project} --memory <MiB>"),
        ];
        return d;
    }
    if cgroups
        && cgroup::pids_limit_hits(&proj_cg).unwrap_or(0) > 0
        && logs_mention(
            &recent,
            &["resource temporarily unavailable", "cannot fork", "eagain"],
        )
    {
        let mut d = base(
            codes::PIDS_LIMIT,
            Severity::Error,
            project,
            Some(name),
            format!("Service {name} hit the project's process/thread limit."),
        );
        d.details = json!({ "exit": exit_json });
        d.suggested_actions = vec![format!(
            "Administrator: nexus project limits {project} --pids <N>"
        )];
        return d;
    }
    if let Some((total, _, avail, _)) =
        resources::statvfs(&nexus_core::paths::data().to_string_lossy())
        && total > 0
        && (avail * 100 / total < 1 || logs_mention(&recent, &["no space left on device"]))
    {
        let mut d = base(
            codes::DISK_FULL,
            Severity::Critical,
            project,
            Some(name),
            "The data disk is full; services cannot write files.".into(),
        );
        d.details = json!({ "exit": exit_json, "available_bytes": avail, "total_bytes": total });
        d.suggested_actions = vec![
            "Free disk space: remove old data in the project's shared directory.".into(),
            "Check `nexus resources` for storage usage.".into(),
        ];
        return d;
    }
    let port_hint = logs_mention(
        &recent,
        &["address already in use", "eaddrinuse", "address in use"],
    );
    if (port_hint || ran_for < Duration::from_secs(10))
        && let Some((port, details)) = port_conflict_details(spec)
        && (port_hint || details["owner_project"].as_str() != Some(project))
    {
        let mut d = base(
            codes::PORT_IN_USE,
            Severity::Error,
            project,
            Some(name),
            format!("The requested port {port} is already occupied."),
        );
        d.details = json!({ "exit": exit_json, "conflict": details });
        d.suggested_actions = vec![format!(
            "Stop the process holding port {port}, or change the service's port in nexus.toml."
        )];
        return d;
    }
    if info.code == Some(127)
        || logs_mention(&recent, &["not found"])
            && info.code.is_some_and(|c| c != 0)
            && ran_for < Duration::from_secs(2)
    {
        let mut d = base(
            codes::EXEC_NOT_FOUND,
            Severity::Error,
            project,
            Some(name),
            format!(
                "The command for service {name} was not found: {}",
                spec.command
            ),
        );
        d.details = json!({ "exit": exit_json, "command": spec.command });
        d.suggested_actions = vec![
            "Check the command path in nexus.toml (relative to the release directory).".into(),
            "Make sure the build step produced the binary, or include it in the upload.".into(),
        ];
        return d;
    }
    if info.code == Some(126)
        || logs_mention(
            &recent,
            &["permission denied", "eacces", "operation not permitted"],
        )
    {
        let mut d = base(
            codes::PERMISSION_DENIED,
            Severity::Error,
            project,
            Some(name),
            format!("Service {name} failed with a permission error."),
        );
        d.details = json!({ "exit": exit_json, "hint": "Services run as an unprivileged user; ports below 1024 and files outside the project are not accessible." });
        d.suggested_actions = vec![
            "Use a port >= 1024.".into(),
            "Write data under $NEXUS_DATA_DIR, not the release directory or system paths.".into(),
            "Ensure the command file is executable (chmod +x before upload).".into(),
        ];
        return d;
    }
    let mem = resources::memory();
    let pressure = mem.total_bytes > 0 && mem.available_bytes * 100 / mem.total_bytes < 5;
    let (code, msg) = match (info.code, info.signal) {
        (_, Some(sig)) => (
            codes::KILLED_BY_SIGNAL,
            format!("Service {name} was killed by signal {sig}."),
        ),
        (Some(c), _) => (
            codes::EXITED_NONZERO,
            format!("Service {name} exited with status {c}."),
        ),
        _ => (codes::EXITED_NONZERO, format!("Service {name} exited.")),
    };
    let mut d = base(code, Severity::Error, project, Some(name), msg);
    d.details = json!({ "exit": exit_json, "system_memory_pressure": pressure });
    d.suggested_actions =
        vec!["Read the service log lines above for the application's own error.".into()];
    if pressure {
        d.suggested_actions
            .push("The system is low on memory (MEMORY_PRESSURE); check `nexus resources`.".into());
    }
    d
}

pub fn spawn_error(
    project: &str,
    name: &str,
    spec: &Service,
    cwd: &Path,
    e: &std::io::Error,
) -> Diagnostic {
    let (code, msg) = match e.kind() {
        std::io::ErrorKind::NotFound => (
            codes::EXEC_NOT_FOUND,
            format!(
                "Cannot start {name}: working directory {} does not exist.",
                cwd.display()
            ),
        ),
        std::io::ErrorKind::PermissionDenied => (
            codes::PERMISSION_DENIED,
            format!("Cannot start {name}: permission denied ({e})."),
        ),
        _ => (codes::SPAWN_FAILED, format!("Cannot start {name}: {e}")),
    };
    let mut d = base(code, Severity::Error, project, Some(name), msg);
    d.details = json!({ "command": spec.command, "workdir": cwd, "os_error": e.to_string() });
    d
}

pub fn crash_loop(project: &str, name: &str, last: Diagnostic, count: usize) -> Diagnostic {
    let mut d = base(
        codes::CRASH_LOOP,
        Severity::Critical,
        project,
        Some(name),
        format!(
            "Service {name} failed {count} times within {} minutes; automatic restarts stopped.",
            crate::state::CRASH_WINDOW.as_secs() / 60
        ),
    );
    d.suggested_actions = last.suggested_actions.clone();
    d.suggested_actions
        .push(format!("After fixing the cause: nexus start {project}"));
    d.details = json!({ "last_failure": last });
    d
}

pub fn health(project: &str, name: &str, spec: &Service, detail: &str) -> Diagnostic {
    let mut d = base(
        codes::HEALTH_CHECK_FAILED,
        Severity::Warning,
        project,
        Some(name),
        format!("Health check for {name} is failing: {detail}"),
    );
    d.details = json!({ "check": spec.health, "detail": detail });
    d.suggested_actions = vec![
        "Confirm the service listens on the configured port/path.".into(),
        "Health checks connect to 127.0.0.1; bind to 0.0.0.0 or 127.0.0.1.".into(),
    ];
    d
}

pub fn dependency(project: &str, name: &str, dep: &str) -> Diagnostic {
    let mut d = base(
        codes::DEPENDENCY_FAILED,
        Severity::Error,
        project,
        Some(name),
        format!("Service {name} is waiting for dependency {dep}, which is not running."),
    );
    d.details = json!({ "dependency": dep });
    d.suggested_actions = vec![format!(
        "Investigate {dep} first: nexus logs {project} --service {dep}"
    )];
    d
}

pub fn manifest_invalid(project: &str, err: &str) -> Diagnostic {
    let mut d = base(
        codes::MANIFEST_INVALID,
        Severity::Error,
        project,
        None,
        format!("nexus.toml is invalid: {err}"),
    );
    d.suggested_actions =
        vec!["Fix nexus.toml and deploy again (see documentation/app-integration.md).".into()];
    d
}

pub fn no_release(project: &str) -> Diagnostic {
    let mut d = base(
        codes::NO_RELEASE,
        Severity::Info,
        project,
        None,
        format!("Project {project} has no deployed release yet."),
    );
    d.recovery_actions = vec![action(
        "deploy",
        format!("tar -czf - . | ssh <server> nexus deploy {project}"),
        "deploy",
    )];
    d
}

pub fn deploy_failure(
    project: &str,
    code: &str,
    message: String,
    logs: Vec<String>,
    details: Value,
) -> Diagnostic {
    let mut d = base(code, Severity::Error, project, None, message);
    d.logs = logs;
    d.details = details;
    d.recovery_actions = vec![
        action("view_history", format!("nexus history {project}"), "read"),
        action("rollback", format!("nexus rollback {project}"), "deploy"),
    ];
    d
}
