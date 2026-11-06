//! Crash-safe file writes. State on /data must survive power loss: ext4
//! keeps new file data in memory for up to ~30 s, so a file that is written
//! and renamed without fsync can come back empty after a hard reset.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Replace `path` atomically with `content` (mode `mode`): write a temp file
/// next to it, fsync it, rename it over `path`, fsync the directory.
pub fn write_durable(path: &Path, content: &[u8], mode: u32) -> io::Result<()> {
    let tmp = path.with_extension("nexus-tmp");
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(content)?;
        f.sync_all()?;
    }
    // The open mode is subject to the umask; set it exactly.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    fs::rename(&tmp, path)?;
    sync_parent(path)
}

/// fsync the directory containing `path` so a new or renamed entry persists.
pub fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => File::open(d)?.sync_all(),
        _ => File::open(".")?.sync_all(),
    }
}

/// Flush every pending write of the filesystem holding `path` (used before
/// activating a freshly extracted release, which has many files).
pub fn syncfs(path: &Path) -> io::Result<()> {
    let f = File::open(path)?;
    if unsafe { libc::syncfs(f.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_content_and_sets_mode() {
        let dir = std::env::temp_dir().join(format!("nexus-fsutil-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("state.toml");
        fs::write(&p, "old").unwrap();
        write_durable(&p, b"new", 0o600).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!dir.join("state.nexus-tmp").exists());
        syncfs(&dir).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
