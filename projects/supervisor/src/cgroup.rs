//! cgroup v2 management: /sys/fs/cgroup/nexus/<project>/<service>.
//!
//! Project-level limits (memory, CPU, tasks) apply to all services of the
//! project together; each service gets its own leaf group so its memory and
//! OOM events can be attributed and the whole service can be killed.

use nexus_core::manifest::Limits;
use std::fs;
use std::path::{Path, PathBuf};

pub fn root() -> PathBuf {
    nexus_core::paths::cgroup_root()
}

pub fn project_dir(project: &str) -> PathBuf {
    root().join(project)
}

pub fn service_dir(project: &str, service: &str) -> PathBuf {
    project_dir(project).join(service)
}

fn write(path: &Path, value: &str) -> std::io::Result<()> {
    fs::write(path, value)
}

fn enable_controllers(dir: &Path) {
    let avail = fs::read_to_string(dir.join("cgroup.controllers")).unwrap_or_default();
    for c in ["cpu", "memory", "pids", "io"] {
        if avail.split_whitespace().any(|x| x == c) {
            let _ = write(&dir.join("cgroup.subtree_control"), &format!("+{c}"));
        }
    }
}

/// Prepare the NEXUS subtree. Returns false when cgroup v2 is not usable
/// (e.g. unprivileged development run); supervision then works without
/// resource limits and reports isolation as limited.
pub fn setup() -> bool {
    let r = root();
    let Some(parent) = r.parent() else {
        return false;
    };
    if !parent.join("cgroup.controllers").exists() {
        return false;
    }
    enable_controllers(parent);
    if fs::create_dir_all(&r).is_err() {
        return false;
    }
    enable_controllers(&r);
    r.join("cgroup.procs").exists()
}

pub fn ensure_project(project: &str, limits: &Limits) -> std::io::Result<()> {
    let d = project_dir(project);
    fs::create_dir_all(&d)?;
    enable_controllers(&d);
    apply_limits(project, limits);
    Ok(())
}

pub fn apply_limits(project: &str, limits: &Limits) {
    let d = project_dir(project);
    let mem = if limits.memory_mb == 0 {
        "max".to_string()
    } else {
        (limits.memory_mb * 1024 * 1024).to_string()
    };
    let _ = write(&d.join("memory.max"), &mem);
    let pids = if limits.pids == 0 {
        "max".to_string()
    } else {
        limits.pids.to_string()
    };
    let _ = write(&d.join("pids.max"), &pids);
    let cpu = if limits.cpu_percent == 0 {
        "max 100000".to_string()
    } else {
        format!("{} 100000", limits.cpu_percent * 1000)
    };
    let _ = write(&d.join("cpu.max"), &cpu);
}

/// Create a leaf group for a service or build and return its path.
pub fn ensure_leaf(project: &str, leaf: &str) -> std::io::Result<PathBuf> {
    let d = service_dir(project, leaf);
    fs::create_dir_all(&d)?;
    // Kill the whole service when the OOM killer picks one of its tasks.
    let _ = write(&d.join("memory.oom.group"), "1");
    Ok(d)
}

pub fn procs(dir: &Path) -> Vec<i32> {
    fs::read_to_string(dir.join("cgroup.procs"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

pub fn signal_all(dir: &Path, sig: i32) {
    for pid in procs(dir) {
        unsafe {
            libc::kill(pid, sig);
        }
    }
}

pub fn kill_all(dir: &Path) {
    if write(&dir.join("cgroup.kill"), "1").is_err() {
        signal_all(dir, libc::SIGKILL);
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn keyed(path: &Path, key: &str) -> Option<u64> {
    fs::read_to_string(path).ok()?.lines().find_map(|l| {
        let (k, v) = l.split_once(' ')?;
        (k == key).then(|| v.trim().parse().ok()).flatten()
    })
}

pub fn memory_current(dir: &Path) -> Option<u64> {
    read_u64(&dir.join("memory.current"))
}

pub fn memory_max(dir: &Path) -> Option<u64> {
    read_u64(&dir.join("memory.max"))
}

pub fn oom_kills(dir: &Path) -> Option<u64> {
    keyed(&dir.join("memory.events"), "oom_kill")
}

pub fn pids_current(dir: &Path) -> Option<u64> {
    read_u64(&dir.join("pids.current"))
}

/// Number of times a fork failed because pids.max was reached.
pub fn pids_limit_hits(dir: &Path) -> Option<u64> {
    keyed(&dir.join("pids.events"), "max")
}

pub fn cpu_usage_usec(dir: &Path) -> Option<u64> {
    keyed(&dir.join("cpu.stat"), "usage_usec")
}

/// Remove a group and its children after killing everything inside.
pub fn remove(dir: &Path) {
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                remove(&e.path());
            }
        }
    }
    if !procs(dir).is_empty() {
        kill_all(dir);
        for _ in 0..50 {
            if procs(dir).is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    let _ = fs::remove_dir(dir);
}

/// Kill processes left over from a previous nexusd instance.
pub fn reap_orphans() -> usize {
    let mut n = 0;
    let Ok(projects) = fs::read_dir(root()) else {
        return 0;
    };
    for p in projects.flatten() {
        let Ok(leaves) = fs::read_dir(p.path()) else {
            continue;
        };
        for l in leaves.flatten() {
            if l.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let k = procs(&l.path()).len();
                if k > 0 {
                    kill_all(&l.path());
                    n += k;
                }
            }
        }
    }
    n
}
