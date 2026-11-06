//! Creation and removal of service identities (`nx-<project>`) and agent
//! accounts (`agent-<name>`) in /etc/passwd, /etc/group and /etc/shadow.
//!
//! On NEXUS those files are symlinks into /data/etc; edits are written to a
//! temporary file next to the real target and renamed over it atomically,
//! under an exclusive lock.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const PROJECT_UID_MIN: u32 = 2000;
pub const PROJECT_UID_MAX: u32 = 2999;
pub const AGENT_UID_MIN: u32 = 3000;
pub const AGENT_UID_MAX: u32 = 3999;

fn etc(name: &str) -> PathBuf {
    let root = std::env::var_os("NEXUS_ROOT").unwrap_or_else(|| "/".into());
    PathBuf::from(root).join("etc").join(name)
}

struct Lock(File);

impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0), libc::LOCK_UN);
        }
    }
}

fn lock() -> io::Result<Lock> {
    let real = fs::canonicalize(etc("passwd"))?;
    let path = real.with_file_name(".pwd.lock");
    let f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&f), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Lock(f))
}

/// Atomically replace the file behind `link` (following symlinks).
fn replace(link: &Path, content: &str) -> io::Result<()> {
    let real = fs::canonicalize(link)?;
    let mode = fs::metadata(&real)?.permissions().mode() & 0o7777;
    let tmp = real.with_extension("nexus-tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, &real)
}

fn used_ids(passwd: &str, group: &str) -> Vec<u32> {
    let mut ids: Vec<u32> = passwd
        .lines()
        .filter_map(|l| l.split(':').nth(2)?.parse().ok())
        .collect();
    ids.extend(
        group
            .lines()
            .filter_map(|l| l.split(':').nth(2)?.parse::<u32>().ok()),
    );
    ids
}

fn has_entry(text: &str, name: &str) -> bool {
    text.lines().any(|l| l.split(':').next() == Some(name))
}

pub struct NewAccount<'a> {
    pub name: &'a str,
    pub gecos: &'a str,
    pub home: &'a str,
    pub shell: &'a str,
    pub uid_range: (u32, u32),
    /// Extra supplementary groups the user joins.
    pub groups: &'a [&'a str],
}

/// Create a user with a same-named primary group and no password.
/// Returns the allocated uid (== gid).
pub fn create(a: &NewAccount) -> io::Result<u32> {
    let _l = lock()?;
    let passwd = fs::read_to_string(etc("passwd"))?;
    let group = fs::read_to_string(etc("group"))?;
    let shadow = fs::read_to_string(etc("shadow"))?;
    if has_entry(&passwd, a.name) || has_entry(&group, a.name) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("account {} already exists", a.name),
        ));
    }
    let used = used_ids(&passwd, &group);
    let id = (a.uid_range.0..=a.uid_range.1)
        .find(|i| !used.contains(i))
        .ok_or_else(|| io::Error::other("no free uid in range"))?;
    let days = nexus_core::time::now_unix() / 86_400;

    let mut new_group = String::with_capacity(group.len() + 64);
    for l in group.lines() {
        let mut fields: Vec<String> = l.split(':').map(String::from).collect();
        if fields.len() >= 4 && a.groups.contains(&fields[0].as_str()) {
            let mut members: Vec<&str> = fields[3].split(',').filter(|m| !m.is_empty()).collect();
            members.push(a.name);
            fields[3] = members.join(",");
        }
        new_group.push_str(&fields.join(":"));
        new_group.push('\n');
    }
    new_group.push_str(&format!("{}:x:{id}:\n", a.name));

    let mut new_passwd = passwd.clone();
    if !new_passwd.ends_with('\n') && !new_passwd.is_empty() {
        new_passwd.push('\n');
    }
    new_passwd.push_str(&format!(
        "{}:x:{id}:{id}:{}:{}:{}\n",
        a.name, a.gecos, a.home, a.shell
    ));
    let mut new_shadow = shadow.clone();
    if !new_shadow.ends_with('\n') && !new_shadow.is_empty() {
        new_shadow.push('\n');
    }
    // "*": no password can ever match; key-based logins still work.
    new_shadow.push_str(&format!("{}:*:{days}:0:99999:7:::\n", a.name));

    replace(&etc("group"), &new_group)?;
    replace(&etc("shadow"), &new_shadow)?;
    replace(&etc("passwd"), &new_passwd)?;
    Ok(id)
}

/// Remove a user, its primary group and its supplementary memberships.
pub fn remove(name: &str) -> io::Result<()> {
    let _l = lock()?;
    let drop_line = |text: &str| -> String {
        text.lines()
            .filter(|l| l.split(':').next() != Some(name))
            .map(|l| format!("{l}\n"))
            .collect()
    };
    let group: String = fs::read_to_string(etc("group"))?
        .lines()
        .filter(|l| l.split(':').next() != Some(name))
        .map(|l| {
            let mut f: Vec<String> = l.split(':').map(String::from).collect();
            if f.len() >= 4 {
                f[3] = f[3]
                    .split(',')
                    .filter(|m| !m.is_empty() && *m != name)
                    .collect::<Vec<_>>()
                    .join(",");
            }
            format!("{}\n", f.join(":"))
        })
        .collect();
    replace(
        &etc("passwd"),
        &drop_line(&fs::read_to_string(etc("passwd"))?),
    )?;
    replace(
        &etc("shadow"),
        &drop_line(&fs::read_to_string(etc("shadow"))?),
    )?;
    replace(&etc("group"), &group)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_remove() {
        let root = std::env::temp_dir().join(format!("nexus-acct-{}", std::process::id()));
        let etcdir = root.join("etc");
        fs::create_dir_all(&etcdir).unwrap();
        fs::write(etcdir.join("passwd"), "root:x:0:0:root:/root:/bin/sh\n").unwrap();
        fs::write(etcdir.join("group"), "root:x:0:\nnexus-svc:x:991:\n").unwrap();
        fs::write(etcdir.join("shadow"), "root:*:1:0:99999:7:::\n").unwrap();
        // SAFETY: tests in this module are the only users of NEXUS_ROOT.
        unsafe { std::env::set_var("NEXUS_ROOT", &root) };
        let a = NewAccount {
            name: "nx-web",
            gecos: "NEXUS project web",
            home: "/data/nexus/projects/web/home",
            shell: "/bin/false",
            uid_range: (PROJECT_UID_MIN, PROJECT_UID_MAX),
            groups: &["nexus-svc"],
        };
        assert_eq!(create(&a).unwrap(), 2000);
        assert!(create(&a).is_err());
        let g = fs::read_to_string(etcdir.join("group")).unwrap();
        assert!(g.contains("nexus-svc:x:991:nx-web"));
        assert!(g.contains("nx-web:x:2000:"));
        assert!(
            fs::read_to_string(etcdir.join("shadow"))
                .unwrap()
                .contains("nx-web:*:")
        );
        remove("nx-web").unwrap();
        let g = fs::read_to_string(etcdir.join("group")).unwrap();
        assert!(!g.contains("nx-web"));
        assert!(
            !fs::read_to_string(etcdir.join("passwd"))
                .unwrap()
                .contains("nx-web")
        );
        unsafe { std::env::remove_var("NEXUS_ROOT") };
        fs::remove_dir_all(&root).unwrap();
    }
}
