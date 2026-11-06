//! Caller identification (SO_PEERCRED), project-scoped authorization and
//! the audit trail.
//!
//! Roles:
//!   * root and members of `wheel` are administrators: every operation,
//!     destructive ones still need explicit confirmation from the CLI.
//!   * everyone else (agents, project identities) is denied unless a grant
//!     in /data/etc/nexus/access.json gives them a permission on a project.

use nexus_core::users;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;

pub const PERMISSIONS: [&str; 7] = [
    "read", "logs", "upload", "build", "deploy", "restart", "config",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub uid: u32,
    pub name: String,
    pub admin: bool,
}

pub fn peer(stream: &UnixStream) -> std::io::Result<Caller> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if r != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(caller_for_uid(cred.uid))
}

pub fn caller_for_uid(uid: u32) -> Caller {
    let user = users::by_uid(uid);
    let name = user
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| format!("uid:{uid}"));
    // The daemon's own uid counts as administrator so an unprivileged
    // development instance is usable by the developer who started it.
    let own = uid == unsafe { libc::geteuid() };
    let admin = uid == 0 || own || user.map(|u| users::in_group(&u, "wheel")).unwrap_or(false);
    Caller { uid, name, admin }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Grants {
    #[serde(default)]
    pub version: u32,
    /// user -> project -> permissions
    #[serde(default)]
    pub grants: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

pub fn load_grants() -> Grants {
    fs::read_to_string(nexus_core::paths::grants())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_grants(g: &Grants) -> std::io::Result<()> {
    let path = nexus_core::paths::grants();
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    nexus_core::fsutil::write_durable(&path, serde_json::to_string_pretty(g)?.as_bytes(), 0o600)
}

/// Normalise a permission list; "all" expands to every permission.
pub fn normalise(perms: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for p in perms {
        let p = p.trim().to_lowercase();
        if p == "all" {
            out.extend(PERMISSIONS.iter().map(|s| s.to_string()));
        } else if PERMISSIONS.contains(&p.as_str()) {
            out.push(p);
        } else {
            return Err(format!(
                "unknown permission {p:?} (valid: {}, all)",
                PERMISSIONS.join(", ")
            ));
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

pub fn allowed(caller: &Caller, project: &str, perm: &str) -> bool {
    if caller.admin {
        return true;
    }
    let g = load_grants();
    g.grants
        .get(&caller.name)
        .and_then(|projects| projects.get(project).or_else(|| projects.get("*")))
        .map(|perms| perms.iter().any(|p| p == perm))
        .unwrap_or(false)
}

/// Append one JSON line to the audit log (root-only, append-only usage).
pub fn audit(caller: &Caller, action: &str, project: Option<&str>, result: &str, detail: Value) {
    let path = nexus_core::paths::audit_log();
    if let Some(d) = path.parent() {
        let _ = fs::create_dir_all(d);
    }
    let rec = json!({
        "ts": nexus_core::time::now_rfc3339(),
        "uid": caller.uid,
        "user": caller.name,
        "action": action,
        "project": project,
        "result": result,
        "detail": detail,
    });
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)
    {
        // The audit trail must survive a power cut right after the action.
        let _ = f.write_all(format!("{rec}\n").as_bytes());
        let _ = f.sync_data();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_normalisation() {
        assert_eq!(
            normalise(&["Logs".into(), "read".into(), "logs".into()]).unwrap(),
            vec!["logs", "read"]
        );
        assert_eq!(normalise(&["all".into()]).unwrap().len(), PERMISSIONS.len());
        assert!(normalise(&["root".into()]).is_err());
    }

    #[test]
    fn non_admin_denied_without_grant() {
        let c = Caller {
            uid: 4242,
            name: "agent-x".into(),
            admin: false,
        };
        assert!(!allowed(&c, "nonexistent-project-xyz", "read"));
        let a = Caller {
            uid: 0,
            name: "root".into(),
            admin: true,
        };
        assert!(allowed(&a, "anything", "deploy"));
    }
}
