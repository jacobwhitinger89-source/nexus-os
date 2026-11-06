//! Read-only lookups in /etc/passwd and /etc/group.

use std::fs;
use std::path::PathBuf;

fn etc_file(name: &str) -> PathBuf {
    let root = std::env::var_os("NEXUS_ROOT").unwrap_or_else(|| "/".into());
    PathBuf::from(root).join("etc").join(name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
}

fn parse_passwd_line(line: &str) -> Option<User> {
    let f: Vec<&str> = line.split(':').collect();
    if f.len() < 7 {
        return None;
    }
    Some(User {
        name: f[0].to_string(),
        uid: f[2].parse().ok()?,
        gid: f[3].parse().ok()?,
        home: f[5].to_string(),
        shell: f[6].to_string(),
    })
}

pub fn all_users() -> Vec<User> {
    fs::read_to_string(etc_file("passwd"))
        .unwrap_or_default()
        .lines()
        .filter_map(parse_passwd_line)
        .collect()
}

pub fn by_uid(uid: u32) -> Option<User> {
    all_users().into_iter().find(|u| u.uid == uid)
}

pub fn by_name(name: &str) -> Option<User> {
    all_users().into_iter().find(|u| u.name == name)
}

/// (group name, gid, members)
pub fn all_groups() -> Vec<(String, u32, Vec<String>)> {
    fs::read_to_string(etc_file("group"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            if f.len() < 4 {
                return None;
            }
            let members = f[3]
                .split(',')
                .filter(|m| !m.is_empty())
                .map(String::from)
                .collect();
            Some((f[0].to_string(), f[2].parse().ok()?, members))
        })
        .collect()
}

pub fn group_gid(name: &str) -> Option<u32> {
    all_groups().into_iter().find(|g| g.0 == name).map(|g| g.1)
}

/// True if `user` is a member of `group` (primary or supplementary).
pub fn in_group(user: &User, group: &str) -> bool {
    all_groups().into_iter().any(|(name, gid, members)| {
        name == group && (gid == user.gid || members.iter().any(|m| m == &user.name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwd_line() {
        let u = parse_passwd_line("admin:x:1000:1000:Admin:/home/admin:/bin/sh").unwrap();
        assert_eq!(u.uid, 1000);
        assert_eq!(u.home, "/home/admin");
        assert!(parse_passwd_line("broken:x").is_none());
    }
}
