//! Deployments: upload → build → atomic activation → health verification →
//! success, or automatic rollback to the previous release.
//!
//! Invariants:
//!   * the previous working release is never deleted before the new one has
//!     passed its health checks (pruning only runs after success and always
//!     keeps the current and last successful release);
//!   * activation is a single rename(2) of the `current` symlink;
//!   * extraction and builds run as the project user inside the sandbox.

use crate::auth::Caller;
use crate::diagnose;
use crate::logs;
use crate::sandbox;
use crate::state::{self, Ctx, ProjectPaths};
use nexus_core::ipc::{Reply, write_reply};
use nexus_core::manifest::Manifest;
use nexus_core::schema::{
    DeployState, DeploymentRecord, Diagnostic, Health, ReleaseInfo, ServiceState, codes,
};
use nexus_core::time;
use serde_json::{Value, json};
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Maximum accepted source archive size.
pub const MAX_UPLOAD: u64 = 2 * 1024 * 1024 * 1024;

fn record_path(project: &str, id: &str) -> std::path::PathBuf {
    ProjectPaths::new(project)
        .deployments()
        .join(format!("{id}.json"))
}

pub fn read_record(project: &str, id: &str) -> Option<DeploymentRecord> {
    serde_json::from_str(&fs::read_to_string(record_path(project, id)).ok()?).ok()
}

fn write_record(r: &DeploymentRecord) {
    let p = record_path(&r.project, &r.id);
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = nexus_core::fsutil::write_durable(
        &p,
        serde_json::to_string_pretty(r)
            .unwrap_or_default()
            .as_bytes(),
        0o644,
    );
}

pub fn history(project: &str) -> Vec<DeploymentRecord> {
    let mut v: Vec<DeploymentRecord> = fs::read_dir(ProjectPaths::new(project).deployments())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .filter_map(|e| serde_json::from_str(&fs::read_to_string(e.path()).ok()?).ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a: &DeploymentRecord, b| b.id.cmp(&a.id));
    v
}

fn random_suffix() -> String {
    let mut b = [0u8; 2];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    format!("{:02x}{:02x}", b[0], b[1])
}

fn send(out: &mut dyn Write, stream: &str, line: impl Into<String>) {
    let _ = write_reply(
        out,
        &Reply::Log {
            stream: stream.into(),
            line: line.into(),
        },
    );
}

/// Point `current` at `releases/<id>` with one atomic rename.
pub fn switch_current(project: &str, id: &str) -> io::Result<()> {
    let pp = ProjectPaths::new(project);
    // The release's files must be on disk before `current` points at them,
    // or a power cut could leave the project running truncated files.
    nexus_core::fsutil::syncfs(&pp.dir)?;
    let tmp = pp.dir.join("current.tmp");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(Path::new("releases").join(id), &tmp)?;
    fs::rename(&tmp, pp.current())?;
    if let Ok(d) = fs::File::open(&pp.dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

fn clear_current(project: &str) {
    let _ = fs::remove_file(ProjectPaths::new(project).current());
}

/// Stop all services of the project and wait for them to exit.
fn stop_and_wait(ctx: &Ctx, project: &str) {
    let max = {
        let mut d = ctx.lock();
        state::set_want(&mut d, project, None, false);
        d.projects
            .get(project)
            .map(|p| {
                p.services
                    .values()
                    .map(|s| s.spec.stop_timeout)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    };
    ctx.poke();
    let deadline = Instant::now() + Duration::from_secs(max + 5);
    while Instant::now() < deadline {
        if ctx
            .lock()
            .projects
            .get(project)
            .map(|p| p.all_stopped())
            .unwrap_or(true)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Switch to `id` with `manifest` and start it.
fn activate(ctx: &Ctx, project: &str, id: &str, manifest: Manifest, by: &str) -> io::Result<()> {
    stop_and_wait(ctx, project);
    switch_current(project, id)?;
    let mut d = ctx.lock();
    if let Some(p) = d.projects.get_mut(project) {
        p.set_manifest(manifest);
        p.release = Some(ReleaseInfo {
            id: id.into(),
            deployed_at: time::now_rfc3339(),
            deployed_by: by.into(),
        });
    }
    state::set_want(&mut d, project, None, true);
    drop(d);
    ctx.poke();
    Ok(())
}

/// Wait until every service is running and not unhealthy.
#[allow(clippy::result_large_err)]
fn wait_healthy(ctx: &Ctx, project: &str, timeout: Duration) -> Result<(), Diagnostic> {
    let start = Instant::now();
    loop {
        {
            let d = ctx.lock();
            let Some(p) = d.projects.get(project) else {
                return Err(diagnose::deploy_failure(
                    project,
                    codes::DEPLOY_HEALTH_FAILED,
                    "project disappeared during deployment".into(),
                    vec![],
                    Value::Null,
                ));
            };
            let bad = p.services.values().find(|s| {
                matches!(s.state, ServiceState::Backoff | ServiceState::Failed)
                    || s.health == Health::Unhealthy
            });
            if let Some(s) = bad {
                let diag = s.diagnostic.clone();
                let msg = format!(
                    "service {} failed after activation ({})",
                    s.spec.name,
                    diag.as_ref().map(|d| d.code.as_str()).unwrap_or("unknown")
                );
                return Err(diagnose::deploy_failure(
                    project,
                    codes::DEPLOY_HEALTH_FAILED,
                    msg,
                    logs::tail_service(project, &s.spec.name, 30),
                    json!({ "service": s.spec.name, "cause": diag }),
                ));
            }
            let ready = p
                .services
                .values()
                .all(|s| matches!(s.state, ServiceState::Running | ServiceState::Exited));
            if ready {
                return Ok(());
            }
            if start.elapsed() > timeout {
                let states: Value = p
                    .services
                    .values()
                    .map(|s| {
                        (
                            s.spec.name.clone(),
                            json!({"state": s.state, "health": s.health}),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>()
                    .into();
                return Err(diagnose::deploy_failure(
                    project,
                    codes::DEPLOY_HEALTH_FAILED,
                    format!(
                        "services did not become healthy within {}s",
                        timeout.as_secs()
                    ),
                    vec![],
                    json!({ "services": states }),
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Run a command as the project user, streaming merged output lines to
/// `on_line`. Kills the whole process group on timeout.
fn run_streamed(
    ctx: &Ctx,
    project: &str,
    mut cmd: Command,
    leaf: &str,
    timeout: Duration,
    mut on_line: impl FnMut(&str),
) -> Result<(), String> {
    let policy = ctx.lock().projects.get(project).map(|p| p.policy.clone());
    let Some(policy) = policy else {
        return Err("project not found".into());
    };
    let sb = {
        let d = ctx.lock();
        state::sandbox_for(&d, &policy, leaf)
    };
    let (reader, writer) = io::pipe().map_err(|e| e.to_string())?;
    let w2 = writer.try_clone().map_err(|e| e.to_string())?;
    cmd.stdout(writer).stderr(w2).stdin(Stdio::null());
    sandbox::apply(&mut cmd, &sb);
    let mut child = cmd.spawn().map_err(|e| format!("cannot start: {e}"))?;
    drop(cmd);
    let pid = child.id() as i32;
    let (tx, rx) = mpsc::channel::<String>();
    let t = std::thread::spawn(move || {
        let r = BufReader::new(reader);
        for line in r.split(b'\n') {
            let Ok(line) = line else { break };
            if tx
                .send(String::from_utf8_lossy(&line).into_owned())
                .is_err()
            {
                break;
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => on_line(&line),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() > deadline && !timed_out {
            timed_out = true;
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            if let Some(cg) = &sb.cgroup {
                crate::cgroup::kill_all(cg);
            }
        }
    }
    let _ = t.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(cg) = &sb.cgroup {
        crate::cgroup::kill_all(cg);
    }
    if timed_out {
        return Err(format!("timed out after {}s", timeout.as_secs()));
    }
    if status.success() {
        Ok(())
    } else {
        Err(format!("exited with {status}"))
    }
}

struct Guard<'a> {
    ctx: &'a Ctx,
    project: String,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.ctx.lock().projects.get_mut(&self.project) {
            p.deploying = false;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn deploy(
    ctx: &Ctx,
    caller: &Caller,
    project: &str,
    compression: &str,
    message: Option<String>,
    allow_build: bool,
    upload: &mut dyn Read,
    out: &mut dyn Write,
) -> Reply {
    let (policy, previous) = {
        let mut d = ctx.lock();
        let Some(p) = d.projects.get_mut(project) else {
            return Reply::err("NOT_FOUND", format!("no project named {project}"));
        };
        if p.deploying {
            return Reply::err(
                "CONFLICT",
                "a deployment of this project is already running",
            );
        }
        p.deploying = true;
        (p.policy.clone(), p.release.as_ref().map(|r| r.id.clone()))
    };
    let _guard = Guard {
        ctx,
        project: project.to_string(),
    };
    let pp = ProjectPaths::new(project);
    let now = time::now_unix();
    let id = format!("{}-{}", time::compact(now), random_suffix());
    let mut rec = DeploymentRecord {
        id: id.clone(),
        project: project.into(),
        status: DeployState::Uploading,
        started_at: time::rfc3339(now),
        finished_at: None,
        deployed_by: caller.name.clone(),
        message,
        previous_release: previous.clone(),
        source_bytes: 0,
        error: None,
    };
    write_record(&rec);
    let fail = |rec: &mut DeploymentRecord, diag: Diagnostic, out: &mut dyn Write| -> Reply {
        rec.status = DeployState::Failed;
        rec.finished_at = Some(time::now_rfc3339());
        rec.error = Some(diag.clone());
        write_record(rec);
        send(out, "deploy", format!("FAILED: {}", diag.message));
        Reply::err_details(
            &diag.code,
            diag.message.clone(),
            json!({ "deployment": rec }),
        )
    };

    // 1. Upload + extract as the project user.
    let rel = pp.release(&id);
    if let Err(e) = fs::create_dir_all(&rel) {
        let d = diagnose::deploy_failure(
            project,
            codes::UPLOAD_FAILED,
            format!("cannot create release dir: {e}"),
            vec![],
            Value::Null,
        );
        return fail(&mut rec, d, out);
    }
    if sandbox::is_root() {
        let _ = chown(&rel, Some(policy.uid), Some(policy.uid));
    }
    let _ = fs::set_permissions(&rel, fs::Permissions::from_mode(0o750));
    send(out, "deploy", format!("release {id}: receiving source"));
    let mut tar = Command::new("tar");
    tar.arg("-x");
    if compression == "gzip" {
        tar.arg("-z");
    }
    tar.args(["-f", "-", "-C"])
        .arg(&rel)
        .env_clear()
        .env("PATH", "/usr/bin:/bin");
    tar.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let sb = {
        let d = ctx.lock();
        state::sandbox_for(&d, &policy, "upload")
    };
    sandbox::apply(&mut tar, &sb);
    let extracted = (|| -> Result<u64, String> {
        let mut child = tar.spawn().map_err(|e| format!("cannot run tar: {e}"))?;
        let mut stdin = child.stdin.take().unwrap();
        let mut total: u64 = 0;
        let mut buf = vec![0u8; 64 * 1024];
        let copy_err = loop {
            let n = match upload.read(&mut buf) {
                Ok(0) => break None,
                Ok(n) => n,
                Err(e) => break Some(format!("upload interrupted: {e}")),
            };
            total += n as u64;
            if total > MAX_UPLOAD {
                break Some(format!("upload exceeds {} bytes", MAX_UPLOAD));
            }
            if let Err(e) = stdin.write_all(&buf[..n]) {
                break Some(format!("archive rejected: {e}"));
            }
        };
        drop(stdin);
        let o = child.wait_with_output().map_err(|e| e.to_string())?;
        if let Some(cg) = &sb.cgroup {
            crate::cgroup::kill_all(cg);
        }
        if let Some(e) = copy_err {
            return Err(e);
        }
        if !o.status.success() {
            return Err(format!(
                "tar failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        if total == 0 {
            return Err("no source archive received on stdin".into());
        }
        Ok(total)
    })();
    match extracted {
        Ok(n) => {
            rec.source_bytes = n;
            send(out, "deploy", format!("received {} bytes", n));
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&rel);
            let d = diagnose::deploy_failure(project, codes::UPLOAD_FAILED, e, vec![], Value::Null);
            return fail(&mut rec, d, out);
        }
    }

    // 2. Manifest.
    let manifest = match state::load_manifest(project, &id) {
        Ok(m) => m,
        Err(e) => {
            let d = diagnose::manifest_invalid(project, &e);
            return fail(&mut rec, d, out);
        }
    };
    if let Some(n) = &manifest.project.name
        && n != project
    {
        let d = diagnose::manifest_invalid(
            project,
            &format!("nexus.toml names project {n:?} but is being deployed to {project:?}"),
        );
        return fail(&mut rec, d, out);
    }

    // 3. Build.
    if manifest.build.is_some() && !allow_build {
        let _ = fs::remove_dir_all(&rel);
        let d = diagnose::deploy_failure(
            project,
            "FORBIDDEN",
            format!(
                "nexus.toml has a [build] section but {} lacks the \"build\" permission; upload prebuilt artifacts or ask an administrator",
                caller.name
            ),
            vec![],
            Value::Null,
        );
        return fail(&mut rec, d, out);
    }
    if let Some(b) = &manifest.build {
        rec.status = DeployState::Building;
        write_record(&rec);
        send(out, "deploy", format!("building: {}", b.command));
        let log_path = pp.deployments().join(format!("{id}.build.log"));
        let mut log = fs::File::create(&log_path).ok();
        let mut tail: Vec<String> = Vec::new();
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&b.command).current_dir(&rel).env_clear();
        for (k, v) in state::base_env(&policy, &rel) {
            cmd.env(k, v);
        }
        for (k, v) in &policy.env {
            cmd.env(k, v);
        }
        let r = run_streamed(
            ctx,
            project,
            cmd,
            "build",
            Duration::from_secs(b.timeout),
            |line| {
                if let Some(f) = log.as_mut() {
                    let _ = writeln!(f, "{line}");
                }
                tail.push(line.to_string());
                if tail.len() > 40 {
                    tail.remove(0);
                }
                send(out, "build", line);
            },
        );
        if let Err(e) = r {
            let d = diagnose::deploy_failure(
                project,
                codes::BUILD_FAILED,
                format!("build command {e}"),
                tail,
                json!({ "command": b.command, "build_log": log_path }),
            );
            return fail(&mut rec, d, out);
        }
        send(out, "deploy", "build succeeded");
    }

    // 4. Activate and verify.
    rec.status = DeployState::Activating;
    write_record(&rec);
    send(out, "deploy", "activating release (restarting services)");
    if let Err(e) = activate(ctx, project, &id, manifest.clone(), &caller.name) {
        let d = diagnose::deploy_failure(
            project,
            codes::SPAWN_FAILED,
            format!("activation failed: {e}"),
            vec![],
            Value::Null,
        );
        return fail(&mut rec, d, out);
    }
    send(
        out,
        "deploy",
        format!(
            "waiting up to {}s for services to become healthy",
            manifest.deploy.health_timeout
        ),
    );
    match wait_healthy(
        ctx,
        project,
        Duration::from_secs(manifest.deploy.health_timeout),
    ) {
        Ok(()) => {
            rec.status = DeployState::Succeeded;
            rec.finished_at = Some(time::now_rfc3339());
            write_record(&rec);
            prune(project, &id, manifest.deploy.keep_releases);
            send(out, "deploy", format!("release {id} is live"));
            Reply::ok(json!({ "deployment": rec }))
        }
        Err(diag) => {
            send(
                out,
                "deploy",
                format!("health verification failed: {}", diag.message),
            );
            rec.error = Some(diag.clone());
            rec.finished_at = Some(time::now_rfc3339());
            let rolled = match &previous {
                Some(prev) => {
                    send(out, "deploy", format!("rolling back to {prev}"));
                    match state::load_manifest(project, prev) {
                        Ok(m) => activate(ctx, project, prev, m, &caller.name).is_ok(),
                        Err(_) => false,
                    }
                }
                None => {
                    stop_and_wait(ctx, project);
                    clear_current(project);
                    let mut d = ctx.lock();
                    if let Some(p) = d.projects.get_mut(project) {
                        p.release = None;
                    }
                    false
                }
            };
            rec.status = if rolled {
                DeployState::RolledBack
            } else {
                DeployState::Failed
            };
            write_record(&rec);
            if rolled {
                send(out, "deploy", "previous release restored");
            }
            Reply::err_details(
                &diag.code,
                diag.message.clone(),
                json!({ "deployment": rec, "rolled_back": rolled }),
            )
        }
    }
}

/// Activate an earlier successful release (default: the one before current).
pub fn rollback(ctx: &Ctx, caller: &Caller, project: &str, to: Option<String>) -> Reply {
    let current = {
        let mut d = ctx.lock();
        let Some(p) = d.projects.get_mut(project) else {
            return Reply::err("NOT_FOUND", format!("no project named {project}"));
        };
        if p.deploying {
            return Reply::err("CONFLICT", "a deployment of this project is running");
        }
        p.deploying = true;
        p.release.as_ref().map(|r| r.id.clone())
    };
    let _guard = Guard {
        ctx,
        project: project.to_string(),
    };
    let target = match to {
        Some(t) => t,
        None => match history(project).into_iter().find(|r| {
            r.status == DeployState::Succeeded
                && Some(&r.id) != current.as_ref()
                && ProjectPaths::new(project).release(&r.id).is_dir()
        }) {
            Some(r) => r.id,
            None => {
                return Reply::err("NOT_FOUND", "no earlier successful release to roll back to");
            }
        },
    };
    if target.is_empty()
        || !target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Reply::err("INVALID", "invalid release id");
    }
    if !ProjectPaths::new(project).release(&target).is_dir() {
        return Reply::err("NOT_FOUND", format!("release {target} is not on disk"));
    }
    let manifest = match state::load_manifest(project, &target) {
        Ok(m) => m,
        Err(e) => return Reply::err(codes::MANIFEST_INVALID, e),
    };
    let timeout = Duration::from_secs(manifest.deploy.health_timeout);
    if let Err(e) = activate(ctx, project, &target, manifest, &caller.name) {
        return Reply::err("FAILED", format!("activation failed: {e}"));
    }
    let now = time::now_rfc3339();
    let rec = DeploymentRecord {
        id: format!("{}-rollback", time::compact(time::now_unix())),
        project: project.into(),
        status: DeployState::Succeeded,
        started_at: now.clone(),
        finished_at: Some(now),
        deployed_by: caller.name.clone(),
        message: Some(format!("rollback to {target}")),
        previous_release: current,
        source_bytes: 0,
        error: None,
    };
    write_record(&rec);
    match wait_healthy(ctx, project, timeout) {
        Ok(()) => Reply::ok(json!({ "rolled_back_to": target, "record": rec })),
        Err(d) => Reply::err_details(
            &d.code,
            format!("rolled back to {target}, but: {}", d.message),
            json!({ "rolled_back_to": target }),
        ),
    }
}

/// Remove old releases, keeping the current, the last successful and the
/// `keep` newest.
fn prune(project: &str, current: &str, keep: usize) {
    let pp = ProjectPaths::new(project);
    let hist = history(project);
    let last_good: Vec<String> = hist
        .iter()
        .filter(|r| r.status == DeployState::Succeeded)
        .take(2)
        .map(|r| r.id.clone())
        .collect();
    let mut ids: Vec<String> = fs::read_dir(pp.releases())
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.reverse();
    for (i, id) in ids.iter().enumerate() {
        if id == current || last_good.contains(id) || i < keep {
            continue;
        }
        let _ = fs::remove_dir_all(pp.release(id));
    }
}
