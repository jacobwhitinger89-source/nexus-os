//! Request handling for one client connection.

use crate::accounts::{self, NewAccount};
use crate::auth::{self, Caller};
use crate::state::{self, Ctx, ProjectPaths};
use crate::{cgroup, deploy, diagnose, logs, sandbox};
use nexus_core::ipc::{Reply, Request, write_reply};
use nexus_core::manifest::{Limits, Policy};
use nexus_core::schema::{self, Severity};
use nexus_core::{paths, resources, time, users};
use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{PermissionsExt, chown};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub const MAX_PROJECT_NAME: usize = 28;

pub fn handle(ctx: Ctx, stream: UnixStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(600)));
    let caller = match auth::peer(&stream) {
        Ok(c) => c,
        Err(_) => return,
    };
    let Ok(mut out) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    // Bound the request line: 64 KiB is far beyond any legitimate request.
    if (&mut reader).take(64 * 1024).read_line(&mut line).is_err() {
        return;
    }
    let reply = match serde_json::from_str::<Request>(&line) {
        Ok(req) => dispatch(&ctx, &caller, req, &mut reader, &mut out),
        Err(e) => Reply::err("INVALID", format!("malformed request: {e}")),
    };
    let _ = write_reply(&mut out, &reply);
}

fn forbidden(caller: &Caller, action: &str, project: Option<&str>, perm: &str) -> Reply {
    auth::audit(
        caller,
        action,
        project,
        "denied",
        json!({ "permission": perm }),
    );
    Reply::err(
        "FORBIDDEN",
        match project {
            Some(p) => format!(
                "user {} lacks the {perm:?} permission on project {p}",
                caller.name
            ),
            None => format!("{action} requires an administrator (member of wheel)"),
        },
    )
}

fn require(caller: &Caller, action: &str, project: &str, perm: &str) -> Result<(), Reply> {
    if auth::allowed(caller, project, perm) {
        Ok(())
    } else {
        Err(forbidden(caller, action, Some(project), perm))
    }
}

fn require_admin(caller: &Caller, action: &str) -> Result<(), Reply> {
    if caller.admin {
        Ok(())
    } else {
        Err(forbidden(caller, action, None, "admin"))
    }
}

fn exists(ctx: &Ctx, project: &str) -> Result<(), Reply> {
    if ctx.lock().projects.contains_key(project) {
        Ok(())
    } else {
        Err(Reply::err(
            "NOT_FOUND",
            format!("no project named {project}"),
        ))
    }
}

fn audited(caller: &Caller, action: &str, project: Option<&str>, detail: Value, r: Reply) -> Reply {
    let result = match &r {
        Reply::Result { ok: true, .. } => "ok",
        _ => "error",
    };
    auth::audit(caller, action, project, result, detail);
    r
}

macro_rules! try_reply {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

fn dispatch(
    ctx: &Ctx,
    caller: &Caller,
    req: Request,
    upload: &mut dyn Read,
    out: &mut UnixStream,
) -> Reply {
    match req {
        Request::Ping => Reply::ok(json!({ "pong": true, "version": nexus_core::VERSION })),
        Request::Whoami => {
            let g = auth::load_grants();
            Reply::ok(json!({
                "user": caller.name, "uid": caller.uid, "admin": caller.admin,
                "grants": g.grants.get(&caller.name),
            }))
        }
        Request::Status => status(ctx, caller),
        Request::Projects => {
            let d = ctx.lock();
            let list: Vec<Value> = d
                .projects
                .iter()
                .filter(|(n, _)| auth::allowed(caller, n, "read"))
                .map(|(n, p)| serde_json::to_value(state::summary(n, p)).unwrap_or_default())
                .collect();
            Reply::ok(schema::envelope("projects", json!({ "projects": list })))
        }
        Request::ProjectStatus { project } => {
            try_reply!(require(caller, "project.status", &project, "read"));
            let mut d = ctx.lock();
            match state::project_status(&mut d, &project) {
                Some(s) => Reply::ok(serde_json::to_value(s).unwrap_or_default()),
                None => Reply::err("NOT_FOUND", format!("no project named {project}")),
            }
        }
        Request::ProjectCreate {
            project,
            memory_mb,
            cpu_percent,
            pids,
        } => {
            try_reply!(require_admin(caller, "project.create"));
            let r = create_project(ctx, &project, memory_mb, cpu_percent, pids);
            audited(caller, "project.create", Some(&project), Value::Null, r)
        }
        Request::ProjectDelete { project, confirm } => {
            try_reply!(require_admin(caller, "project.delete"));
            if confirm != project {
                return Reply::err(
                    "CONFIRMATION_REQUIRED",
                    format!(
                        "deleting a project erases its releases and data; confirm by passing the project name ({project})"
                    ),
                );
            }
            let r = delete_project(ctx, &project);
            audited(caller, "project.delete", Some(&project), Value::Null, r)
        }
        Request::Start { project, service } => {
            control(ctx, caller, &project, service.as_deref(), "start")
        }
        Request::Stop { project, service } => {
            control(ctx, caller, &project, service.as_deref(), "stop")
        }
        Request::Restart { project, service } => {
            control(ctx, caller, &project, service.as_deref(), "restart")
        }
        Request::Logs {
            project,
            service,
            errors,
            lines,
        } => {
            try_reply!(require(caller, "logs", &project, "logs"));
            try_reply!(exists(ctx, &project));
            let lines = lines.clamp(1, 5000);
            let services: Vec<String> = ctx.lock().projects[&project].order();
            let out_lines = if errors {
                logs::tail_errors(&project, lines)
                    .into_iter()
                    .filter(|l| {
                        service
                            .as_ref()
                            .is_none_or(|s| l.contains(&format!("[{s}]")))
                    })
                    .collect()
            } else {
                match &service {
                    Some(s) if services.contains(s) => logs::tail_service(&project, s, lines),
                    Some(s) => return Reply::err("NOT_FOUND", format!("no service named {s}")),
                    None => logs::tail_merged(&project, &services, lines),
                }
            };
            Reply::ok(schema::envelope(
                "logs",
                json!({ "project": project, "service": service, "errors_only": errors, "lines": out_lines }),
            ))
        }
        Request::Doctor { project } => {
            try_reply!(require(caller, "doctor", &project, "read"));
            try_reply!(exists(ctx, &project));
            doctor(ctx, &project)
        }
        Request::Deploy {
            project,
            compression,
            message,
        } => {
            try_reply!(require(caller, "deploy", &project, "upload"));
            try_reply!(require(caller, "deploy", &project, "deploy"));
            if compression != "none" && compression != "gzip" {
                return Reply::err("INVALID", "compression must be none or gzip");
            }
            // Builds run arbitrary commands: callers without "build" may
            // only ship prebuilt artifacts (enforced once nexus.toml is read).
            let allow_build = auth::allowed(caller, &project, "build");
            let r = deploy::deploy(
                ctx,
                caller,
                &project,
                &compression,
                message,
                allow_build,
                upload,
                out,
            );
            audited(caller, "deploy", Some(&project), Value::Null, r)
        }
        Request::Rollback { project, to } => {
            try_reply!(require(caller, "rollback", &project, "deploy"));
            let r = deploy::rollback(ctx, caller, &project, to.clone());
            audited(caller, "rollback", Some(&project), json!({ "to": to }), r)
        }
        Request::History { project } => {
            try_reply!(require(caller, "history", &project, "read"));
            try_reply!(exists(ctx, &project));
            let h = deploy::history(&project);
            Reply::ok(schema::envelope(
                "deploy_history",
                json!({ "project": project, "deployments": h }),
            ))
        }
        Request::EnvSet {
            project,
            key,
            value,
        } => {
            try_reply!(require(caller, "env.set", &project, "config"));
            if key.is_empty()
                || key.starts_with("NEXUS_")
                || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Reply::err(
                    "INVALID",
                    "variable names use A-Z, 0-9, _ and may not start with NEXUS_",
                );
            }
            let r = update_policy(ctx, &project, |p| {
                p.env.insert(key.clone(), value.clone());
            });
            audited(caller, "env.set", Some(&project), json!({ "key": key }), r)
        }
        Request::EnvUnset { project, key } => {
            try_reply!(require(caller, "env.unset", &project, "config"));
            let r = update_policy(ctx, &project, |p| {
                p.env.remove(&key);
            });
            audited(
                caller,
                "env.unset",
                Some(&project),
                json!({ "key": key }),
                r,
            )
        }
        Request::EnvList { project } => {
            try_reply!(require(caller, "env.list", &project, "config"));
            let d = ctx.lock();
            match d.projects.get(&project) {
                // Values are secrets: only names are ever returned.
                Some(p) => Reply::ok(
                    json!({ "project": project, "keys": p.policy.env.keys().collect::<Vec<_>>() }),
                ),
                None => Reply::err("NOT_FOUND", format!("no project named {project}")),
            }
        }
        Request::LimitsSet {
            project,
            memory_mb,
            cpu_percent,
            pids,
        } => {
            try_reply!(require_admin(caller, "limits.set"));
            let r = update_policy(ctx, &project, |p| {
                if let Some(m) = memory_mb {
                    p.limits.memory_mb = m;
                }
                if let Some(c) = cpu_percent {
                    p.limits.cpu_percent = c;
                }
                if let Some(n) = pids {
                    p.limits.pids = n;
                }
            });
            if matches!(r, Reply::Result { ok: true, .. }) {
                let d = ctx.lock();
                if d.cgroups
                    && let Some(p) = d.projects.get(&project)
                {
                    cgroup::apply_limits(&project, &p.policy.limits);
                }
            }
            audited(
                caller,
                "limits.set",
                Some(&project),
                json!({ "memory_mb": memory_mb, "cpu_percent": cpu_percent, "pids": pids }),
                r,
            )
        }
        Request::AccessGrant {
            user,
            project,
            permissions,
        } => {
            try_reply!(require_admin(caller, "access.grant"));
            if users::by_name(&user).is_none() {
                return Reply::err("NOT_FOUND", format!("no user named {user}"));
            }
            if project != "*" {
                try_reply!(exists(ctx, &project));
            }
            let perms = match auth::normalise(&permissions) {
                Ok(p) => p,
                Err(e) => return Reply::err("INVALID", e),
            };
            let mut g = auth::load_grants();
            g.version = 1;
            g.grants
                .entry(user.clone())
                .or_default()
                .insert(project.clone(), perms.clone());
            let r = match auth::save_grants(&g) {
                Ok(()) => {
                    Reply::ok(json!({ "user": user, "project": project, "permissions": perms }))
                }
                Err(e) => Reply::err("FAILED", e.to_string()),
            };
            audited(
                caller,
                "access.grant",
                Some(&project),
                json!({ "user": user, "permissions": perms }),
                r,
            )
        }
        Request::AccessRevoke { user, project } => {
            try_reply!(require_admin(caller, "access.revoke"));
            let mut g = auth::load_grants();
            if let Some(m) = g.grants.get_mut(&user) {
                m.remove(&project);
                if m.is_empty() {
                    g.grants.remove(&user);
                }
            }
            let r = match auth::save_grants(&g) {
                Ok(()) => Reply::ok(json!({ "user": user, "project": project, "revoked": true })),
                Err(e) => Reply::err("FAILED", e.to_string()),
            };
            audited(
                caller,
                "access.revoke",
                Some(&project),
                json!({ "user": user }),
                r,
            )
        }
        Request::AccessList => {
            try_reply!(require_admin(caller, "access.list"));
            Reply::ok(serde_json::to_value(auth::load_grants()).unwrap_or_default())
        }
        Request::Audit { lines } => {
            try_reply!(require_admin(caller, "audit.read"));
            let l = logs::tail_file(&paths::audit_log(), lines.clamp(1, 5000));
            let entries: Vec<Value> = l
                .iter()
                .filter_map(|x| serde_json::from_str(x).ok())
                .collect();
            Reply::ok(schema::envelope("audit", json!({ "entries": entries })))
        }
        Request::AgentCreate { name, public_key } => {
            try_reply!(require_admin(caller, "agent.create"));
            let r = agent_create(&name, &public_key);
            audited(caller, "agent.create", None, json!({ "agent": name }), r)
        }
        Request::AgentRemove { name, confirm } => {
            try_reply!(require_admin(caller, "agent.remove"));
            if confirm != name {
                return Reply::err(
                    "CONFIRMATION_REQUIRED",
                    format!("confirm by passing the agent name ({name})"),
                );
            }
            let r = agent_remove(&name);
            audited(caller, "agent.remove", None, json!({ "agent": name }), r)
        }
        Request::AgentList => {
            try_reply!(require_admin(caller, "agent.list"));
            let g = auth::load_grants();
            let agents: Vec<Value> = users::all_users()
                .into_iter()
                .filter(|u| u.name.starts_with("agent-"))
                .map(|u| json!({ "user": u.name, "uid": u.uid, "grants": g.grants.get(&u.name) }))
                .collect();
            Reply::ok(json!({ "agents": agents }))
        }
        Request::SshStatus => Reply::ok(ssh_status()),
        Request::SshFinalize => {
            try_reply!(require_admin(caller, "ssh.finalize"));
            let r = ssh_finalize(caller);
            audited(caller, "ssh.finalize", None, Value::Null, r)
        }
        Request::SshAllowPasswords => {
            try_reply!(require_admin(caller, "ssh.allow_passwords"));
            let r = match set_password_auth(true) {
                Ok(()) => Reply::ok(json!({ "password_auth": true })),
                Err(e) => Reply::err("FAILED", e),
            };
            audited(caller, "ssh.allow_passwords", None, Value::Null, r)
        }
    }
}

fn status(ctx: &Ctx, caller: &Caller) -> Reply {
    let d = ctx.lock();
    let visible: Vec<_> = d
        .projects
        .iter()
        .filter(|(n, _)| auth::allowed(caller, n, "read"))
        .map(|(n, p)| state::summary(n, p))
        .collect();
    let read = |p: &str| {
        fs::read_to_string(paths::run().join(p))
            .ok()
            .map(|s| s.trim().to_string())
    };
    Reply::ok(schema::envelope(
        "status",
        json!({
            "hostname": resources::hostname(),
            "os_version": resources::os_version(),
            "kernel": resources::kernel_release(),
            "uptime_seconds": resources::uptime_seconds(),
            "boot_mode": read("mode"),
            "boot_slot": read("slot"),
            "data_storage": read("data-state"),
            "supervisor": {
                "version": nexus_core::VERSION,
                "cgroups": d.cgroups,
                "isolation": match d.isolation { sandbox::Isolation::Full => "full", sandbox::Isolation::Limited => "limited" },
            },
            "ssh": ssh_status(),
            "projects": visible,
        }),
    ))
}

fn control(ctx: &Ctx, caller: &Caller, project: &str, service: Option<&str>, op: &str) -> Reply {
    try_reply!(require(caller, op, project, "restart"));
    {
        let mut d = ctx.lock();
        let Some(p) = d.projects.get(project) else {
            return Reply::err("NOT_FOUND", format!("no project named {project}"));
        };
        if p.release.is_none() {
            return Reply::err(
                schema::codes::NO_RELEASE,
                format!("project {project} has nothing deployed yet"),
            );
        }
        if let Some(s) = service
            && !p.services.contains_key(s)
        {
            return Reply::err("NOT_FOUND", format!("no service named {s}"));
        }
        if p.deploying {
            return Reply::err("CONFLICT", "a deployment is in progress");
        }
        match op {
            "start" => state::set_want(&mut d, project, service, true),
            _ => state::set_want(&mut d, project, service, false),
        }
    }
    ctx.poke();
    if op == "restart" {
        // Wait for the stop to complete, then start again.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            let stopped = ctx
                .lock()
                .projects
                .get(project)
                .map(|p| {
                    p.services
                        .iter()
                        .filter(|(n, _)| service.is_none_or(|s| s == n.as_str()))
                        .all(|(_, s)| !s.alive())
                })
                .unwrap_or(true);
            if stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut d = ctx.lock();
        state::set_want(&mut d, project, service, true);
        drop(d);
        ctx.poke();
    }
    let r = Reply::ok(
        json!({ "project": project, "service": service, "action": op, "accepted": true }),
    );
    audited(caller, op, Some(project), json!({ "service": service }), r)
}

fn create_project(
    ctx: &Ctx,
    project: &str,
    memory_mb: Option<u64>,
    cpu_percent: Option<u64>,
    pids: Option<u64>,
) -> Reply {
    if !nexus_core::valid_name(project) || project.len() > MAX_PROJECT_NAME {
        return Reply::err(
            "INVALID",
            format!(
                "project names use lowercase letters, digits and '-', start with a letter, max {MAX_PROJECT_NAME} chars"
            ),
        );
    }
    if ctx.lock().projects.contains_key(project) {
        return Reply::err("CONFLICT", format!("project {project} already exists"));
    }
    let pp = ProjectPaths::new(project);
    let user = format!("nx-{project}");
    let root = sandbox::is_root();
    let uid = if root {
        match users::by_name(&user) {
            Some(u) => u.uid,
            None => match accounts::create(&NewAccount {
                name: &user,
                gecos: &format!("NEXUS project {project}"),
                home: &pp.home().to_string_lossy(),
                shell: "/bin/false",
                uid_range: (accounts::PROJECT_UID_MIN, accounts::PROJECT_UID_MAX),
                groups: &["nexus-svc"],
            }) {
                Ok(u) => u,
                Err(e) => return Reply::err("FAILED", format!("cannot create service user: {e}")),
            },
        }
    } else {
        unsafe { libc::geteuid() }
    };
    let mut limits = Limits::default();
    if let Some(m) = memory_mb {
        limits.memory_mb = m;
    }
    if let Some(c) = cpu_percent {
        limits.cpu_percent = c;
    }
    if let Some(n) = pids {
        limits.pids = n;
    }
    let policy = Policy {
        id: format!("prj-{}", time::compact(time::now_unix()).to_lowercase()),
        name: project.into(),
        user: if root {
            user
        } else {
            users::by_uid(uid).map(|u| u.name).unwrap_or_default()
        },
        uid,
        created: time::now_rfc3339(),
        autostart: true,
        limits,
        env: Default::default(),
    };
    let mk = || -> std::io::Result<()> {
        fs::create_dir_all(&pp.dir)?;
        fs::set_permissions(&pp.dir, fs::Permissions::from_mode(0o711))?;
        for (dir, owned, mode) in [
            (pp.releases(), true, 0o750),
            (pp.shared(), true, 0o750),
            (pp.home(), true, 0o750),
            (logs::log_dir(project), false, 0o700),
            (pp.deployments(), false, 0o700),
        ] {
            fs::create_dir_all(&dir)?;
            if owned && root {
                chown(&dir, Some(uid), Some(uid))?;
            }
            fs::set_permissions(&dir, fs::Permissions::from_mode(mode))?;
        }
        let pf = pp.policy();
        fs::write(&pf, policy.to_toml())?;
        fs::set_permissions(&pf, fs::Permissions::from_mode(0o600))?;
        Ok(())
    };
    if let Err(e) = mk() {
        return Reply::err("FAILED", format!("cannot create project directories: {e}"));
    }
    let mut d = ctx.lock();
    if d.cgroups {
        let _ = cgroup::ensure_project(project, &policy.limits);
    }
    match state::Project::load(project) {
        Ok(p) => {
            d.projects.insert(project.into(), p);
            Reply::ok(
                json!({ "project": project, "id": policy.id, "user": policy.user, "uid": uid, "limits": policy.limits,
                "next": format!("tar -czf - . | ssh <server> nexus deploy {project}") }),
            )
        }
        Err(e) => Reply::err("FAILED", e),
    }
}

fn delete_project(ctx: &Ctx, project: &str) -> Reply {
    try_reply!(exists(ctx, project));
    {
        let mut d = ctx.lock();
        if d.projects[project].deploying {
            return Reply::err("CONFLICT", "a deployment is in progress");
        }
        state::set_want(&mut d, project, None, false);
    }
    ctx.poke();
    for _ in 0..600 {
        if ctx.lock().projects[project].all_stopped() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let (user, cgroups) = {
        let mut d = ctx.lock();
        let p = d.projects.remove(project).unwrap();
        (p.policy.user, d.cgroups)
    };
    if cgroups {
        cgroup::remove(&cgroup::project_dir(project));
    }
    let _ = fs::remove_dir_all(ProjectPaths::new(project).dir);
    let _ = fs::remove_dir_all(paths::metrics_dir().join(project));
    if sandbox::is_root() && user.starts_with("nx-") {
        let _ = accounts::remove(&user);
    }
    let mut g = auth::load_grants();
    for m in g.grants.values_mut() {
        m.remove(project);
    }
    g.grants.retain(|_, m| !m.is_empty());
    let _ = auth::save_grants(&g);
    Reply::ok(json!({ "project": project, "deleted": true }))
}

fn update_policy(ctx: &Ctx, project: &str, f: impl FnOnce(&mut Policy)) -> Reply {
    let mut d = ctx.lock();
    let Some(p) = d.projects.get_mut(project) else {
        return Reply::err("NOT_FOUND", format!("no project named {project}"));
    };
    let mut policy = p.policy.clone();
    f(&mut policy);
    let path = ProjectPaths::new(project).policy();
    let tmp = path.with_extension("tmp");
    let w = fs::write(&tmp, policy.to_toml())
        .and_then(|_| fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)))
        .and_then(|_| fs::rename(&tmp, &path));
    match w {
        Ok(()) => {
            p.policy = policy;
            Reply::ok(
                json!({ "project": project, "updated": true, "note": "takes effect when services restart" }),
            )
        }
        Err(e) => Reply::err("FAILED", e.to_string()),
    }
}

fn doctor(ctx: &Ctx, project: &str) -> Reply {
    let mut checks: Vec<Value> = Vec::new();
    let mut diags: Vec<schema::Diagnostic> = Vec::new();
    let mut check = |name: &str, ok: Option<bool>, msg: String| {
        let status = match ok {
            Some(true) => "pass",
            Some(false) => "fail",
            None => "warn",
        };
        checks.push(json!({ "check": name, "status": status, "message": msg }));
    };
    let mut d = ctx.lock();
    let cgroups = d.cgroups;
    let status = state::project_status(&mut d, project);
    let p = &d.projects[project];
    match (&p.release, &p.manifest_error) {
        (None, _) => {
            check("release", Some(false), "no release deployed".into());
            diags.push(diagnose::no_release(project));
        }
        (Some(r), None) => check("release", Some(true), format!("release {} active", r.id)),
        (Some(r), Some(e)) => {
            check("manifest", Some(false), format!("release {}: {e}", r.id));
            diags.push(diagnose::manifest_invalid(project, e));
        }
    }
    let listening = resources::listening_tcp_ports();
    for name in p.order() {
        let s = &p.services[&name];
        let st = format!("{:?}", s.state).to_lowercase();
        let ok = match s.state {
            schema::ServiceState::Running | schema::ServiceState::Exited => Some(true),
            schema::ServiceState::Stopped if !s.want => None,
            schema::ServiceState::Starting | schema::ServiceState::Waiting => None,
            _ => Some(false),
        };
        check(
            &format!("service:{name}"),
            ok,
            format!("state {st}, health {:?}, restarts {}", s.health, s.restarts).to_lowercase(),
        );
        for dep in &s.spec.depends_on {
            if s.want
                && !p.services.get(dep).map(|x| x.ready()).unwrap_or(false)
                && s.state == schema::ServiceState::Waiting
            {
                diags.push(diagnose::dependency(project, &name, dep));
            }
        }
        for port in &s.spec.ports {
            let held = listening.iter().find(|(lp, _)| *lp == port.port());
            match (held, s.alive()) {
                (Some(_), true) => check(
                    &format!("port:{}", port.port()),
                    Some(true),
                    format!("{name} listening"),
                ),
                (None, true) => check(
                    &format!("port:{}", port.port()),
                    None,
                    format!("{name} is running but nothing listens on {}", port.port()),
                ),
                (Some((_, inode)), false) => {
                    let owner = resources::socket_owner(*inode);
                    check(
                        &format!("port:{}", port.port()),
                        Some(false),
                        format!(
                            "port {} is held by {:?} while {name} is not running",
                            port.port(),
                            owner
                        ),
                    );
                }
                (None, false) => {}
            }
        }
        if let Some(diag) = &s.diagnostic {
            diags.push(diag.clone());
        }
    }
    if cgroups {
        let pd = cgroup::project_dir(project);
        let ooms = cgroup::oom_kills(&pd).unwrap_or(0);
        check(
            "memory",
            Some(ooms == 0),
            format!(
                "{} used of {} limit, {ooms} OOM kills",
                cgroup::memory_current(&pd)
                    .map(resources::human_bytes)
                    .unwrap_or("?".into()),
                cgroup::memory_max(&pd)
                    .map(resources::human_bytes)
                    .unwrap_or("no".into())
            ),
        );
        let hits = cgroup::pids_limit_hits(&pd).unwrap_or(0);
        check(
            "tasks",
            Some(hits == 0),
            format!(
                "{} tasks, pids limit reached {hits} times",
                cgroup::pids_current(&pd).unwrap_or(0)
            ),
        );
    } else {
        check(
            "isolation",
            None,
            "cgroups unavailable: no resource limits or per-project accounting".into(),
        );
    }
    if let Some((total, _, avail, _)) = resources::statvfs(&paths::data().to_string_lossy()) {
        let pct = (avail * 100).checked_div(total).unwrap_or(0);
        check(
            "disk",
            Some(pct >= 5),
            format!(
                "{} free ({pct}%) on data storage",
                resources::human_bytes(avail)
            ),
        );
    }
    let mem = resources::memory();
    let mpct = (mem.available_bytes * 100)
        .checked_div(mem.total_bytes)
        .unwrap_or(0);
    check(
        "system_memory",
        Some(mpct >= 5),
        format!(
            "{} available ({mpct}%)",
            resources::human_bytes(mem.available_bytes)
        ),
    );
    let recent_errors = logs::tail_errors(project, 20);
    let healthy = diags.iter().all(|d| d.severity <= Severity::Info)
        && checks.iter().all(|c| c["status"] != "fail");
    diags.sort_by_key(|d| std::cmp::Reverse(d.severity));
    diags.dedup_by(|a, b| a.code == b.code && a.service == b.service);
    Reply::ok(schema::envelope(
        "doctor",
        json!({
            "project": project,
            "status": status.as_ref().map(|s| s.status),
            "healthy": healthy,
            "checks": checks,
            "diagnostics": diags,
            "recent_errors": recent_errors,
        }),
    ))
}

fn agent_create(name: &str, key: &str) -> Reply {
    if !nexus_core::valid_name(name) || name.len() > 24 {
        return Reply::err(
            "INVALID",
            "agent names use lowercase letters, digits and '-', max 24 chars",
        );
    }
    let key = key.trim();
    let parts: Vec<&str> = key.split_whitespace().collect();
    let valid_type = parts.first().is_some_and(|t| {
        [
            "ssh-ed25519",
            "ecdsa-sha2-nistp256",
            "ecdsa-sha2-nistp384",
            "ecdsa-sha2-nistp521",
            "ssh-rsa",
            "sk-ssh-ed25519@openssh.com",
        ]
        .contains(t)
    });
    if parts.len() < 2 || !valid_type || key.contains('\n') || key.contains('"') {
        return Reply::err(
            "INVALID",
            "expected one OpenSSH public key line, e.g. 'ssh-ed25519 AAAA... comment'",
        );
    }
    if !sandbox::is_root() {
        return Reply::err("FAILED", "creating accounts requires nexusd to run as root");
    }
    let user = format!("agent-{name}");
    let home = format!("/home/{user}");
    let uid = match accounts::create(&NewAccount {
        name: &user,
        gecos: &format!("NEXUS agent {name}"),
        home: &home,
        shell: "/bin/sh",
        uid_range: (accounts::AGENT_UID_MIN, accounts::AGENT_UID_MAX),
        groups: &["nexus-agent"],
    }) {
        Ok(u) => u,
        Err(e) => return Reply::err("FAILED", e.to_string()),
    };
    let ssh = std::path::Path::new(&home).join(".ssh");
    let w = || -> std::io::Result<()> {
        fs::create_dir_all(&ssh)?;
        // Forced command: the key can only run the nexus CLI.
        let line = format!(
            "command=\"/usr/bin/nexus agent-exec\",no-port-forwarding,no-agent-forwarding,no-X11-forwarding,no-pty {} {}\n",
            parts[0], parts[1]
        );
        fs::write(ssh.join("authorized_keys"), line)?;
        for (p, mode) in [
            (std::path::PathBuf::from(&home), 0o700),
            (ssh.clone(), 0o700),
            (ssh.join("authorized_keys"), 0o600),
        ] {
            chown(&p, Some(uid), Some(uid))?;
            fs::set_permissions(&p, fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    };
    if let Err(e) = w() {
        let _ = accounts::remove(&user);
        return Reply::err("FAILED", format!("cannot write authorized_keys: {e}"));
    }
    Reply::ok(json!({
        "agent": name, "user": user, "uid": uid,
        "next": format!("nexus access grant {user} <project> read,logs,deploy,restart"),
    }))
}

fn agent_remove(name: &str) -> Reply {
    let user = format!("agent-{name}");
    let Some(u) = users::by_name(&user) else {
        return Reply::err("NOT_FOUND", format!("no agent {name}"));
    };
    if let Err(e) = accounts::remove(&user) {
        return Reply::err("FAILED", e.to_string());
    }
    if u.home.starts_with("/home/agent-") {
        let _ = fs::remove_dir_all(&u.home);
    }
    let mut g = auth::load_grants();
    g.grants.remove(&user);
    let _ = auth::save_grants(&g);
    Reply::ok(json!({ "agent": name, "removed": true }))
}

fn ssh_conf() -> std::path::PathBuf {
    paths::etc().join("ssh.conf")
}

fn ssh_status() -> Value {
    let conf = fs::read_to_string(ssh_conf()).unwrap_or_default();
    let pw = conf.lines().any(|l| l.trim() == "PASSWORD_AUTH=yes");
    let fps: Vec<Value> = ["ed25519", "ecdsa"]
        .iter()
        .filter_map(|t| {
            let key = paths::data().join(format!("etc/dropbear/dropbear_{t}_host_key"));
            let out = std::process::Command::new("dropbearkey")
                .arg("-y")
                .arg("-f")
                .arg(&key)
                .output()
                .ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            let fp = text.lines().find_map(|l| l.strip_prefix("Fingerprint: "))?;
            Some(json!({ "type": t, "fingerprint": fp.trim() }))
        })
        .collect();
    json!({ "password_auth": pw, "host_keys": fps })
}

fn set_password_auth(enabled: bool) -> Result<(), String> {
    let path = ssh_conf();
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("PASSWORD_AUTH="))
        .map(String::from)
        .collect();
    lines.push(format!(
        "PASSWORD_AUTH={}",
        if enabled { "yes" } else { "no" }
    ));
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    restart_sshd();
    Ok(())
}

/// Restart only the listening dropbear (child of init); sessions survive.
fn restart_sshd() {
    let Ok(rd) = fs::read_dir("/proc") else {
        return;
    };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else {
            continue;
        };
        let st = fs::read_to_string(e.path().join("status")).unwrap_or_default();
        let name = st
            .lines()
            .find_map(|l| l.strip_prefix("Name:"))
            .map(str::trim);
        let ppid = st
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .map(str::trim);
        if name == Some("dropbear") && ppid == Some("1") {
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
    }
}

/// Key-login evidence for `user` in the system log.
pub fn key_login_seen(user: &str) -> bool {
    let needle = format!("Pubkey auth succeeded for '{user}'");
    let log = paths::data().join("log/messages");
    (0..=4).any(|i| {
        let p = if i == 0 {
            log.clone()
        } else {
            log.with_extension(format!("{}", i - 1))
        };
        fs::read_to_string(p)
            .map(|t| t.contains(&needle))
            .unwrap_or(false)
    })
}

fn ssh_finalize(caller: &Caller) -> Reply {
    if !key_login_seen(&caller.name) {
        return Reply::err(
            "NOT_VERIFIED",
            format!(
                "no successful SSH key login for {} has been recorded yet. Log in once with your key (ssh -o PasswordAuthentication=no {}@<host>) and run this again from that session.",
                caller.name, caller.name
            ),
        );
    }
    match set_password_auth(false) {
        Ok(()) => Reply::ok(
            json!({ "password_auth": false, "note": "SSH now accepts keys only. Local console login still works." }),
        ),
        Err(e) => Reply::err("FAILED", e),
    }
}
