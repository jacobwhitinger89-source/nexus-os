//! Supervisor state and the monitor loop.
//!
//! All mutable state lives in one `Daemon` behind a mutex. The monitor
//! thread wakes every 250 ms (or when signalled), reaps exited services,
//! starts services whose dependencies are ready, escalates stops and applies
//! restart policies. Request handlers only flip desired state (`want`) and
//! wake the monitor; they never block on processes while holding the lock.

use crate::sandbox::{self, Isolation, Sandbox};
use crate::{cgroup, diagnose, logs};
use nexus_core::manifest::{Manifest, Policy, RestartPolicy, Service};
use nexus_core::schema::{
    Diagnostic, ExitInfo, Health, PortStatus, ProjectResources, ProjectState, ProjectStatus,
    ProjectSummary, ReleaseInfo, SCHEMA_VERSION, ServiceState, ServiceStatus,
};
use nexus_core::{paths, resources, time};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

pub const CRASH_WINDOW: Duration = Duration::from_secs(300);
pub const CRASH_LIMIT: usize = 10;
const STABLE_AFTER: Duration = Duration::from_secs(60);
const STARTED_AFTER: Duration = Duration::from_secs(2);

pub struct Shared {
    pub d: Mutex<Daemon>,
    pub wake: Condvar,
}

pub type Ctx = Arc<Shared>;

impl Shared {
    pub fn lock(&self) -> MutexGuard<'_, Daemon> {
        self.d.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn poke(&self) {
        self.wake.notify_all();
    }
}

pub struct Daemon {
    pub projects: BTreeMap<String, Project>,
    pub cgroups: bool,
    pub isolation: Isolation,
    pub shutting_down: bool,
    pub firewall_ports: BTreeSet<(String, u16)>,
}

pub struct Project {
    pub policy: Policy,
    pub manifest: Option<Manifest>,
    pub manifest_error: Option<String>,
    pub release: Option<ReleaseInfo>,
    pub want_running: bool,
    pub services: BTreeMap<String, Svc>,
    pub deploying: bool,
    pub cpu_sample: Option<(Instant, u64)>,
    pub cpu_percent: Option<f64>,
}

pub struct Svc {
    pub spec: Service,
    pub want: bool,
    pub state: ServiceState,
    pub pid: Option<i32>,
    pub started: Option<(Instant, u64)>,
    pub restarts: u32,
    pub consecutive_failures: u32,
    pub failure_times: Vec<Instant>,
    pub next_start: Option<Instant>,
    pub stop_deadline: Option<Instant>,
    pub killed: bool,
    pub last_exit: Option<ExitInfo>,
    pub health: Health,
    pub health_failures: u32,
    pub next_health: Option<Instant>,
    pub health_running: bool,
    pub health_restart: bool,
    pub diagnostic: Option<Diagnostic>,
    pub oom_before: u64,
}

impl Svc {
    pub fn new(spec: Service) -> Svc {
        Svc {
            health: if spec.health.is_some() {
                Health::Starting
            } else {
                Health::None
            },
            spec,
            want: false,
            state: ServiceState::Stopped,
            pid: None,
            started: None,
            restarts: 0,
            consecutive_failures: 0,
            failure_times: Vec::new(),
            next_start: None,
            stop_deadline: None,
            killed: false,
            last_exit: None,
            health_failures: 0,
            next_health: None,
            health_running: false,
            health_restart: false,
            diagnostic: None,
            oom_before: 0,
        }
    }

    pub fn alive(&self) -> bool {
        self.pid.is_some()
    }

    /// Ready to satisfy dependents.
    pub fn ready(&self) -> bool {
        self.state == ServiceState::Running
    }
}

/// Filesystem locations of one project.
pub struct ProjectPaths {
    pub dir: PathBuf,
}

impl ProjectPaths {
    pub fn new(name: &str) -> Self {
        Self {
            dir: paths::project(name),
        }
    }
    pub fn policy(&self) -> PathBuf {
        self.dir.join("project.toml")
    }
    pub fn releases(&self) -> PathBuf {
        self.dir.join("releases")
    }
    pub fn release(&self, id: &str) -> PathBuf {
        self.releases().join(id)
    }
    pub fn current(&self) -> PathBuf {
        self.dir.join("current")
    }
    pub fn shared(&self) -> PathBuf {
        self.dir.join("shared")
    }
    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }
    pub fn deployments(&self) -> PathBuf {
        self.dir.join("deployments")
    }
}

pub fn current_release(name: &str) -> Option<String> {
    let link = fs::read_link(ProjectPaths::new(name).current()).ok()?;
    link.file_name().map(|n| n.to_string_lossy().into_owned())
}

pub fn load_manifest(name: &str, release: &str) -> Result<Manifest, String> {
    let p = ProjectPaths::new(name)
        .release(release)
        .join(nexus_core::manifest::MANIFEST_FILE);
    let text = fs::read_to_string(&p).map_err(|e| format!("cannot read nexus.toml: {e}"))?;
    Manifest::parse(&text)
}

pub fn release_info(name: &str, id: &str) -> ReleaseInfo {
    let rec = crate::deploy::read_record(name, id);
    ReleaseInfo {
        id: id.to_string(),
        deployed_at: rec
            .as_ref()
            .and_then(|r| r.finished_at.clone())
            .unwrap_or_default(),
        deployed_by: rec.map(|r| r.deployed_by).unwrap_or_default(),
    }
}

impl Project {
    pub fn load(name: &str) -> Result<Project, String> {
        let pp = ProjectPaths::new(name);
        let text = fs::read_to_string(pp.policy()).map_err(|e| e.to_string())?;
        let policy = Policy::parse(&text)?;
        let mut p = Project {
            want_running: false,
            policy,
            manifest: None,
            manifest_error: None,
            release: None,
            services: BTreeMap::new(),
            deploying: false,
            cpu_sample: None,
            cpu_percent: None,
        };
        if let Some(rel) = current_release(name) {
            p.release = Some(release_info(name, &rel));
            match load_manifest(name, &rel) {
                Ok(m) => p.set_manifest(m),
                Err(e) => p.manifest_error = Some(e),
            }
            p.want_running = p.policy.autostart && p.manifest.is_some();
        }
        Ok(p)
    }

    /// Replace service definitions. Callers must have stopped the old ones.
    pub fn set_manifest(&mut self, m: Manifest) {
        self.services = m
            .services
            .iter()
            .map(|s| (s.name.clone(), Svc::new(s.clone())))
            .collect();
        self.manifest = Some(m);
        self.manifest_error = None;
    }

    pub fn order(&self) -> Vec<String> {
        self.manifest
            .as_ref()
            .and_then(|m| m.start_order().ok())
            .unwrap_or_else(|| self.services.keys().cloned().collect())
    }

    pub fn all_stopped(&self) -> bool {
        self.services.values().all(|s| !s.alive())
    }

    pub fn state(&self) -> ProjectState {
        if self.deploying {
            return ProjectState::Deploying;
        }
        if self.release.is_none() {
            return ProjectState::NotDeployed;
        }
        if !self.want_running {
            return ProjectState::Inactive;
        }
        let total = self.services.len();
        let ok = self
            .services
            .values()
            .filter(|s| s.state == ServiceState::Running || s.state == ServiceState::Exited)
            .filter(|s| s.health != Health::Unhealthy)
            .count();
        let alive = self.services.values().filter(|s| s.alive()).count();
        if total > 0 && ok == total {
            ProjectState::Active
        } else if alive == 0
            && self
                .services
                .values()
                .any(|s| s.state == ServiceState::Failed || s.state == ServiceState::Backoff)
        {
            ProjectState::Failed
        } else {
            ProjectState::Degraded
        }
    }
}

pub fn new_ctx(cgroups: bool) -> Ctx {
    Arc::new(Shared {
        d: Mutex::new(Daemon {
            projects: BTreeMap::new(),
            cgroups,
            isolation: if cgroups && sandbox::is_root() {
                Isolation::Full
            } else {
                Isolation::Limited
            },
            shutting_down: false,
            firewall_ports: BTreeSet::new(),
        }),
        wake: Condvar::new(),
    })
}

pub fn load_all(ctx: &Ctx) {
    let mut names: Vec<String> = fs::read_dir(paths::projects())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| nexus_core::valid_name(n))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let mut d = ctx.lock();
    for n in names {
        match Project::load(&n) {
            Ok(p) => {
                if d.cgroups {
                    let _ = cgroup::ensure_project(&n, &p.policy.limits);
                }
                if p.want_running {
                    for s in p.services.keys() {
                        logs::event(&n, s, "autostart after boot", false);
                    }
                }
                d.projects.insert(n, p);
            }
            Err(e) => eprintln!("nexusd: skipping project {n}: {e}"),
        }
    }
    let wanted: Vec<String> = d
        .projects
        .iter()
        .filter(|(_, p)| p.want_running)
        .map(|(n, _)| n.clone())
        .collect();
    for n in wanted {
        set_want(&mut d, &n, None, true);
    }
}

/// Request services (all when `service` is None) to run or stop.
pub fn set_want(d: &mut Daemon, project: &str, service: Option<&str>, want: bool) {
    let Some(p) = d.projects.get_mut(project) else {
        return;
    };
    if service.is_none() {
        p.want_running = want;
    } else if want {
        p.want_running = true;
    }
    for (name, s) in p.services.iter_mut() {
        if service.is_some_and(|x| x != name) {
            continue;
        }
        s.want = want;
        if want {
            // An explicit start clears a previous give-up.
            if matches!(
                s.state,
                ServiceState::Failed | ServiceState::Exited | ServiceState::Backoff
            ) {
                s.state = ServiceState::Stopped;
            }
            s.failure_times.clear();
            s.consecutive_failures = 0;
            s.next_start = None;
        }
    }
}

fn exit_info(status: i32) -> ExitInfo {
    let (code, signal) = if libc::WIFEXITED(status) {
        (Some(libc::WEXITSTATUS(status)), None)
    } else if libc::WIFSIGNALED(status) {
        (None, Some(libc::WTERMSIG(status)))
    } else {
        (None, None)
    };
    ExitInfo {
        code,
        signal,
        at: time::now_rfc3339(),
    }
}

/// Build the environment for a project process.
pub fn base_env(policy: &Policy, release_dir: &std::path::Path) -> Vec<(String, String)> {
    let pp = ProjectPaths::new(&policy.name);
    vec![
        (
            "PATH".into(),
            "/usr/local/bin:/usr/bin:/bin:/usr/local/sbin".into(),
        ),
        ("HOME".into(), pp.home().to_string_lossy().into()),
        ("USER".into(), policy.user.clone()),
        ("LOGNAME".into(), policy.user.clone()),
        ("LANG".into(), "C.UTF-8".into()),
        ("TMPDIR".into(), "/tmp".into()),
        ("NEXUS_PROJECT".into(), policy.name.clone()),
        (
            "NEXUS_RELEASE_DIR".into(),
            release_dir.to_string_lossy().into(),
        ),
        (
            "NEXUS_DATA_DIR".into(),
            pp.shared().to_string_lossy().into(),
        ),
    ]
}

pub fn sandbox_for(d: &Daemon, policy: &Policy, leaf: &str) -> Sandbox {
    let cg = if d.cgroups {
        cgroup::ensure_leaf(&policy.name, leaf).ok()
    } else {
        None
    };
    Sandbox {
        uid: policy.uid,
        gid: policy.uid,
        cgroup: cg,
        private_tmp: true,
        nofile: policy.limits.nofile.max(64),
    }
}

fn spawn_service(d: &mut Daemon, project: &str, name: &str) {
    let cgroups = d.cgroups;
    let (policy, release) = {
        let p = &d.projects[project];
        (p.policy.clone(), p.release.clone())
    };
    let Some(release) = release else {
        return;
    };
    let pp = ProjectPaths::new(project);
    let rel_dir = pp.release(&release.id);
    let sb = sandbox_for(d, &policy, name);
    let p = d.projects.get_mut(project).unwrap();
    let s = p.services.get_mut(name).unwrap();
    let spec = s.spec.clone();
    let cwd = match &spec.workdir {
        Some(w) => rel_dir.join(w),
        None => rel_dir.clone(),
    };
    let metrics_dir = paths::metrics_dir().join(project);
    let _ = fs::create_dir_all(&metrics_dir);
    if sandbox::is_root() {
        let _ = std::os::unix::fs::chown(&metrics_dir, Some(policy.uid), Some(policy.uid));
    }

    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(format!("exec {}", spec.command))
        .current_dir(&cwd)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in base_env(&policy, &rel_dir) {
        cmd.env(k, v);
    }
    cmd.env("NEXUS_SERVICE", name)
        .env("NEXUS_RELEASE", &release.id)
        .env(
            "NEXUS_METRICS_FILE",
            metrics_dir.join(format!("{name}.json")),
        );
    if let Some(port) = spec.ports.first() {
        cmd.env("PORT", port.port().to_string());
    }
    for (k, v) in spec.env.iter().chain(policy.env.iter()) {
        cmd.env(k, v);
    }
    sandbox::apply(&mut cmd, &sb);

    if cgroups {
        s.oom_before = cgroup::oom_kills(&cgroup::service_dir(project, name)).unwrap_or(0);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id() as i32;
            if let Some(out) = child.stdout.take() {
                let (pr, sv) = (project.to_string(), name.to_string());
                let _ = std::thread::Builder::new()
                    .stack_size(64 * 1024)
                    .spawn(move || logs::pump(pr, sv, "out", out));
            }
            if let Some(err) = child.stderr.take() {
                let (pr, sv) = (project.to_string(), name.to_string());
                let _ = std::thread::Builder::new()
                    .stack_size(64 * 1024)
                    .spawn(move || logs::pump(pr, sv, "err", err));
            }
            // The child is reaped with waitpid in the monitor loop.
            std::mem::forget(child);
            s.pid = Some(pid);
            s.started = Some((Instant::now(), time::now_unix()));
            s.state = ServiceState::Starting;
            s.stop_deadline = None;
            s.killed = false;
            s.health_restart = false;
            s.health_failures = 0;
            if let Some(h) = &spec.health {
                s.health = Health::Starting;
                s.next_health = Some(Instant::now() + Duration::from_secs(1.min(h.interval)));
            }
            logs::event(project, name, &format!("started (pid {pid})"), false);
        }
        Err(e) => {
            let diag = diagnose::spawn_error(project, name, &spec, &cwd, &e);
            logs::event(project, name, &diag.message, true);
            s.diagnostic = Some(diag);
            s.state = ServiceState::Failed;
        }
    }
}

fn begin_stop(project: &str, name: &str, s: &mut Svc, cgroups: bool) {
    if let Some(pid) = s.pid {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        if cgroups {
            cgroup::signal_all(&cgroup::service_dir(project, name), libc::SIGTERM);
        }
        s.state = ServiceState::Stopping;
        s.stop_deadline = Some(Instant::now() + Duration::from_secs(s.spec.stop_timeout));
        logs::event(project, name, "stopping (SIGTERM)", false);
    } else {
        s.state = ServiceState::Stopped;
    }
}

fn handle_exit(d: &mut Daemon, project: &str, name: &str, status: i32) {
    let cgroups = d.cgroups;
    let p = d.projects.get_mut(project).unwrap();
    let s = p.services.get_mut(name).unwrap();
    let info = exit_info(status);
    let ran_for = s.started.map(|(i, _)| i.elapsed()).unwrap_or_default();
    s.pid = None;
    s.last_exit = Some(info.clone());
    s.stop_deadline = None;
    if cgroups {
        // Never leave stray children of the service behind.
        cgroup::kill_all(&cgroup::service_dir(project, name));
    }
    let desc = match (info.code, info.signal) {
        (Some(c), _) => format!("exited with status {c}"),
        (_, Some(sig)) => format!("killed by signal {sig}"),
        _ => "exited".to_string(),
    };
    if s.state == ServiceState::Stopping || !s.want {
        s.state = ServiceState::Stopped;
        logs::event(project, name, &format!("stopped ({desc})"), false);
        return;
    }
    let spec = s.spec.clone();
    let mut diag = if s.health_restart {
        s.diagnostic.clone().unwrap_or_else(|| {
            diagnose::exit(project, name, &spec, &info, ran_for, cgroups, s.oom_before)
        })
    } else {
        diagnose::exit(project, name, &spec, &info, ran_for, cgroups, s.oom_before)
    };
    s.health_restart = false;
    let clean = info.code == Some(0);
    let restart = match spec.restart {
        RestartPolicy::Never => false,
        RestartPolicy::OnFailure => !clean,
        RestartPolicy::Always => true,
    };
    if clean && !restart {
        s.state = ServiceState::Exited;
        logs::event(project, name, &desc, false);
        return;
    }
    logs::event(project, name, &format!("{desc} — {}", diag.code), !clean);
    if !restart {
        s.state = ServiceState::Failed;
        s.diagnostic = Some(diag);
        return;
    }
    let now = Instant::now();
    s.failure_times
        .retain(|t| now.duration_since(*t) < CRASH_WINDOW);
    s.failure_times.push(now);
    if s.failure_times.len() >= CRASH_LIMIT {
        diag = diagnose::crash_loop(project, name, diag, s.failure_times.len());
        logs::event(project, name, &diag.message, true);
        s.state = ServiceState::Failed;
        s.diagnostic = Some(diag);
        return;
    }
    s.consecutive_failures = if ran_for >= STABLE_AFTER {
        1
    } else {
        s.consecutive_failures + 1
    };
    let exp = s.consecutive_failures.saturating_sub(1).min(6);
    let delay = (spec.restart_delay.max(1) << exp).min(60);
    s.restarts += 1;
    s.next_start = Some(now + Duration::from_secs(delay));
    s.state = ServiceState::Backoff;
    s.diagnostic = Some(diag);
    logs::event(project, name, &format!("restarting in {delay}s"), false);
}

/// One pass of the supervisor.
pub fn tick(d: &mut Daemon) {
    let cgroups = d.cgroups;
    let names: Vec<String> = d.projects.keys().cloned().collect();
    // 1. Reap.
    for pn in &names {
        let svc_pids: Vec<(String, i32)> = d.projects[pn]
            .services
            .iter()
            .filter_map(|(n, s)| s.pid.map(|p| (n.clone(), p)))
            .collect();
        for (sn, pid) in svc_pids {
            let mut status = 0;
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if r == pid {
                handle_exit(d, pn, &sn, status);
            } else if r < 0 {
                // Not our child any more (should not happen); forget it.
                handle_exit(d, pn, &sn, 0);
            }
        }
    }
    // 2. Drive each service towards its desired state.
    let now = Instant::now();
    for pn in &names {
        let order = d.projects[pn].order();
        for sn in &order {
            let p = &d.projects[pn];
            let s = &p.services[sn];
            let deps_ready = s
                .spec
                .depends_on
                .iter()
                .all(|dep| p.services.get(dep).map(|x| x.ready()).unwrap_or(false));
            let dependents_alive = p
                .services
                .values()
                .any(|o| o.alive() && o.spec.depends_on.iter().any(|x| x == sn));
            let release = p.release.is_some() && !p.deploying || p.deploying && p.release.is_some();
            let (want, state) = (s.want, s.state);
            match (want, state) {
                (true, ServiceState::Stopped | ServiceState::Waiting) if release => {
                    if deps_ready {
                        spawn_service(d, pn, sn);
                    } else if state != ServiceState::Waiting {
                        let s = d
                            .projects
                            .get_mut(pn)
                            .unwrap()
                            .services
                            .get_mut(sn)
                            .unwrap();
                        s.state = ServiceState::Waiting;
                    }
                }
                (true, ServiceState::Backoff) => {
                    let due = s.next_start.map(|t| now >= t).unwrap_or(true);
                    if due && deps_ready {
                        spawn_service(d, pn, sn);
                    }
                }
                (true, ServiceState::Starting) => {
                    let up = s.started.map(|(i, _)| i.elapsed()).unwrap_or_default();
                    if s.spec.health.is_none() && up >= STARTED_AFTER {
                        let s = d
                            .projects
                            .get_mut(pn)
                            .unwrap()
                            .services
                            .get_mut(sn)
                            .unwrap();
                        s.state = ServiceState::Running;
                        if up >= STARTED_AFTER {
                            s.diagnostic = None;
                        }
                    }
                }
                (false, ServiceState::Running | ServiceState::Starting) if !dependents_alive => {
                    let s = d
                        .projects
                        .get_mut(pn)
                        .unwrap()
                        .services
                        .get_mut(sn)
                        .unwrap();
                    begin_stop(pn, sn, s, cgroups);
                }
                (false, ServiceState::Waiting | ServiceState::Backoff) => {
                    let s = d
                        .projects
                        .get_mut(pn)
                        .unwrap()
                        .services
                        .get_mut(sn)
                        .unwrap();
                    s.state = ServiceState::Stopped;
                }
                _ => {}
            }
            // Escalate overdue stops (requested or health-triggered).
            let s = d
                .projects
                .get_mut(pn)
                .unwrap()
                .services
                .get_mut(sn)
                .unwrap();
            if let (Some(deadline), Some(pid)) = (s.stop_deadline, s.pid)
                && now >= deadline
                && !s.killed
            {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                if cgroups {
                    cgroup::kill_all(&cgroup::service_dir(pn, sn));
                }
                s.killed = true;
                logs::event(pn, sn, "stop timeout exceeded — sent SIGKILL", true);
            }
        }
    }
}

/// Health-check verdict from the health thread.
pub fn health_result(
    d: &mut Daemon,
    project: &str,
    service: &str,
    pid: i32,
    ok: bool,
    detail: String,
) {
    let cgroups = d.cgroups;
    let Some(s) = d
        .projects
        .get_mut(project)
        .and_then(|p| p.services.get_mut(service))
    else {
        return;
    };
    s.health_running = false;
    if s.pid != Some(pid) {
        return;
    }
    let Some(h) = s.spec.health.clone() else {
        return;
    };
    s.next_health = Some(Instant::now() + Duration::from_secs(h.interval.max(1)));
    if ok {
        if s.health != Health::Healthy {
            logs::event(project, service, "health check passing", false);
        }
        s.health = Health::Healthy;
        s.health_failures = 0;
        if s.state == ServiceState::Starting {
            s.state = ServiceState::Running;
            s.diagnostic = None;
        }
        return;
    }
    let up = s.started.map(|(i, _)| i.elapsed()).unwrap_or_default();
    if up < Duration::from_secs(h.start_period) && s.health != Health::Healthy {
        return;
    }
    s.health_failures += 1;
    if s.health_failures < h.retries {
        return;
    }
    if s.health != Health::Unhealthy {
        logs::event(
            project,
            service,
            &format!("health check failing: {detail}"),
            true,
        );
    }
    s.health = Health::Unhealthy;
    let diag = diagnose::health(project, service, &s.spec, &detail);
    s.diagnostic = Some(diag);
    if s.spec.restart != RestartPolicy::Never && s.stop_deadline.is_none() {
        // Restart the service: terminate it; handle_exit applies the policy.
        s.health_restart = true;
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        if cgroups {
            cgroup::signal_all(&cgroup::service_dir(project, service), libc::SIGTERM);
        }
        s.stop_deadline = Some(Instant::now() + Duration::from_secs(s.spec.stop_timeout));
        logs::event(project, service, "restarting unhealthy service", true);
    }
}

/// Public ports of running services, for the firewall.
pub fn desired_ports(d: &Daemon) -> BTreeSet<(String, u16)> {
    let mut set = BTreeSet::new();
    for p in d.projects.values() {
        for s in p.services.values() {
            if s.alive() {
                for port in &s.spec.ports {
                    if port.public() {
                        set.insert((port.protocol().to_string(), port.port()));
                    }
                }
            }
        }
    }
    set
}

fn read_metrics(project: &str) -> Option<serde_json::Value> {
    let dir = paths::metrics_dir().join(project);
    let mut out = serde_json::Map::new();
    for e in fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(svc) = name.strip_suffix(".json") else {
            continue;
        };
        if e.metadata().map(|m| m.len() > 64 * 1024).unwrap_or(true) {
            continue;
        }
        if let Ok(v) = fs::read_to_string(e.path())
            .map_err(|_| ())
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|_| ()))
            && v.is_object()
        {
            out.insert(svc.to_string(), v);
        }
    }
    (!out.is_empty()).then_some(serde_json::Value::Object(out))
}

pub fn service_status(
    project: &str,
    s: &Svc,
    cgroups: bool,
    listening: &[(u16, u64)],
) -> ServiceStatus {
    ServiceStatus {
        name: s.spec.name.clone(),
        state: s.state,
        health: s.health,
        pid: s.pid.map(|p| p as u32),
        uptime_seconds: s
            .pid
            .and(s.started)
            .map(|(_, t)| time::now_unix().saturating_sub(t)),
        restarts: s.restarts,
        last_exit: s.last_exit.clone(),
        memory_bytes: if cgroups && s.alive() {
            cgroup::memory_current(&cgroup::service_dir(project, &s.spec.name))
        } else {
            None
        },
        ports: s
            .spec
            .ports
            .iter()
            .map(|p| PortStatus {
                port: p.port(),
                protocol: p.protocol().to_string(),
                public: p.public(),
                listening: listening.iter().any(|(lp, _)| *lp == p.port()),
            })
            .collect(),
        depends_on: s.spec.depends_on.clone(),
    }
}

pub fn project_status(d: &mut Daemon, name: &str) -> Option<ProjectStatus> {
    let cgroups = d.cgroups;
    let p = d.projects.get_mut(name)?;
    let listening = resources::listening_tcp_ports();
    let services: Vec<ServiceStatus> = p
        .order()
        .iter()
        .filter_map(|n| p.services.get(n))
        .map(|s| service_status(name, s, cgroups, &listening))
        .collect();
    let pdir = cgroup::project_dir(name);
    if cgroups && let Some(usage) = cgroup::cpu_usage_usec(&pdir) {
        let now = Instant::now();
        if let Some((at, prev)) = p.cpu_sample {
            let dt = now.duration_since(at).as_micros() as f64;
            if dt > 0.0 {
                p.cpu_percent =
                    Some((1000.0 * (usage.saturating_sub(prev)) as f64 / dt).round() / 10.0);
            }
        }
        p.cpu_sample = Some((now, usage));
    }
    let failure = p
        .order()
        .iter()
        .filter_map(|n| p.services.get(n))
        .filter_map(|s| s.diagnostic.clone())
        .max_by_key(|d| d.severity)
        .or_else(|| {
            p.manifest_error
                .as_ref()
                .map(|e| diagnose::manifest_invalid(name, e))
        })
        .or_else(|| p.release.is_none().then(|| diagnose::no_release(name)));
    let logs_available = logs::log_dir(name)
        .read_dir()
        .map(|mut r| r.next().is_some())
        .unwrap_or(false);
    Some(ProjectStatus {
        schema_version: SCHEMA_VERSION,
        project: name.to_string(),
        id: p.policy.id.clone(),
        status: p.state(),
        release: p.release.clone(),
        services,
        diagnostics_available: true,
        failure,
        resources: ProjectResources {
            memory_bytes: cgroups.then(|| cgroup::memory_current(&pdir)).flatten(),
            memory_limit_bytes: cgroups.then(|| cgroup::memory_max(&pdir)).flatten(),
            cpu_percent: if cgroups { p.cpu_percent } else { None },
            pids: cgroups.then(|| cgroup::pids_current(&pdir)).flatten(),
            oom_kills: cgroups.then(|| cgroup::oom_kills(&pdir)).flatten(),
        },
        metrics: read_metrics(name),
        logs_available,
    })
}

pub fn summary(name: &str, p: &Project) -> ProjectSummary {
    ProjectSummary {
        name: name.to_string(),
        id: p.policy.id.clone(),
        status: p.state(),
        services_total: p.services.len(),
        services_running: p.services.values().filter(|s| s.alive()).count(),
        release: p.release.as_ref().map(|r| r.id.clone()),
    }
}
