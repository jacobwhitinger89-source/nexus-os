//! Filesystem layout. Every path can be relocated under `$NEXUS_ROOT`
//! so the components can be tested on a development host without root.

use std::path::PathBuf;

fn root() -> PathBuf {
    std::env::var_os("NEXUS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn under(rel: &str) -> PathBuf {
    root().join(rel)
}

/// Persistent data partition (tmpfs in live mode).
pub fn data() -> PathBuf {
    under("data")
}
/// Mutable system configuration (`/data/etc/nexus`).
pub fn etc() -> PathBuf {
    under("data/etc/nexus")
}
/// Root of all project state.
pub fn projects() -> PathBuf {
    under("data/nexus/projects")
}
pub fn project(name: &str) -> PathBuf {
    projects().join(name)
}
pub fn audit_log() -> PathBuf {
    under("data/nexus/audit/audit.log")
}
/// Access grants for non-administrator users (agents).
pub fn grants() -> PathBuf {
    etc().join("access.json")
}
pub fn run() -> PathBuf {
    under("run/nexus")
}
pub fn socket() -> PathBuf {
    std::env::var_os("NEXUS_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| run().join("nexusd.sock"))
}
/// Per-service metrics files written by applications.
pub fn metrics_dir() -> PathBuf {
    run().join("metrics")
}
pub fn cgroup_root() -> PathBuf {
    std::env::var_os("NEXUS_CGROUP_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup/nexus"))
}
