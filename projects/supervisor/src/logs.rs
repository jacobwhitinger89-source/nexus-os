//! Size-bounded, rotated log files.
//!
//! Layout per project (`/data/nexus/projects/<p>/logs/`, root-only):
//!   <service>.log   stdout + stderr + supervisor events for the service
//!   errors.log      stderr lines and supervisor failures of all services
//! Each line: `<RFC3339> <stream> <text>` where stream is out, err or nexus.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP: usize = 3;
const MAX_LINE: usize = 8 * 1024;

struct Sink {
    path: PathBuf,
    file: Option<File>,
    size: u64,
}

impl Sink {
    fn write(&mut self, line: &str) {
        if self.file.is_none() || self.size > MAX_BYTES {
            if self.size > MAX_BYTES {
                rotate(&self.path);
            }
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
            self.size = self
                .file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map(|m| m.len())
                .unwrap_or(0);
        }
        if let Some(f) = self.file.as_mut()
            && f.write_all(line.as_bytes()).is_ok()
        {
            self.size += line.len() as u64;
        }
    }
}

fn rotate(path: &Path) {
    for i in (1..KEEP).rev() {
        let _ = fs::rename(numbered(path, i), numbered(path, i + 1));
    }
    let _ = fs::rename(path, numbered(path, 1));
}

fn numbered(path: &Path, i: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{i}"));
    PathBuf::from(s)
}

type SinkMap = Mutex<HashMap<PathBuf, Arc<Mutex<Sink>>>>;

fn sinks() -> &'static SinkMap {
    static SINKS: OnceLock<SinkMap> = OnceLock::new();
    SINKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sink(path: PathBuf) -> Arc<Mutex<Sink>> {
    let mut map = sinks().lock().unwrap();
    map.entry(path.clone())
        .or_insert_with(|| {
            Arc::new(Mutex::new(Sink {
                path,
                file: None,
                size: 0,
            }))
        })
        .clone()
}

pub fn log_dir(project: &str) -> PathBuf {
    nexus_core::paths::project(project).join("logs")
}

fn service_log(project: &str, service: &str) -> PathBuf {
    log_dir(project).join(format!("{service}.log"))
}

fn errors_log(project: &str) -> PathBuf {
    log_dir(project).join("errors.log")
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
        .collect()
}

/// Append one line to a service log (and errors.log for err/warning lines).
pub fn append(project: &str, service: &str, stream: &str, text: &str, is_error: bool) {
    let line = format!(
        "{} {stream} {}\n",
        nexus_core::time::now_rfc3339(),
        sanitize(text.trim_end_matches(['\n', '\r']))
    );
    sink(service_log(project, service))
        .lock()
        .unwrap()
        .write(&line);
    if is_error {
        let eline = format!(
            "{} {stream} [{service}] {}\n",
            nexus_core::time::now_rfc3339(),
            sanitize(text.trim_end_matches(['\n', '\r']))
        );
        sink(errors_log(project)).lock().unwrap().write(&eline);
    }
}

/// Supervisor event for a service, e.g. "exited with status 1".
pub fn event(project: &str, service: &str, text: &str, is_error: bool) {
    append(project, service, "nexus", text, is_error);
}

/// Pump a child's pipe into the log until EOF (runs on its own thread).
pub fn pump(project: String, service: String, stream: &'static str, src: impl Read) {
    let mut r = BufReader::with_capacity(4096, src);
    let mut buf = Vec::with_capacity(256);
    loop {
        buf.clear();
        match read_line_bounded(&mut r, &mut buf) {
            Ok(0) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                append(&project, &service, stream, &text, stream == "err");
            }
            Err(_) => break,
        }
    }
}

/// Like read_until('\n') but splits lines longer than MAX_LINE.
fn read_line_bounded(r: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    let mut total = 0;
    loop {
        let avail = r.fill_buf()?;
        if avail.is_empty() {
            return Ok(total);
        }
        let room = MAX_LINE - buf.len();
        let (n, done) = match avail.iter().take(room).position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => {
                let n = avail.len().min(room);
                (n, buf.len() + n >= MAX_LINE)
            }
        };
        buf.extend_from_slice(&avail[..n]);
        r.consume(n);
        total += n;
        if done {
            return Ok(total);
        }
    }
}

/// Last `n` lines of a log file, reading rotated files when needed.
pub fn tail_file(path: &Path, n: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut files = vec![path.to_path_buf()];
    for i in 1..=KEEP {
        files.push(numbered(path, i));
    }
    for f in files {
        if lines.len() >= n {
            break;
        }
        let Ok(text) = fs::read_to_string(&f) else {
            break;
        };
        let mut chunk: Vec<String> = text.lines().map(String::from).collect();
        chunk.append(&mut lines);
        lines = chunk;
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

pub fn tail_service(project: &str, service: &str, n: usize) -> Vec<String> {
    tail_file(&service_log(project, service), n)
}

pub fn tail_errors(project: &str, n: usize) -> Vec<String> {
    tail_file(&errors_log(project), n)
}

/// Merge the last `n` lines of several service logs by timestamp.
pub fn tail_merged(project: &str, services: &[String], n: usize) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for s in services {
        for l in tail_service(project, s, n) {
            // Insert the service name after the timestamp and stream.
            let mut parts = l.splitn(3, ' ');
            let (ts, stream, rest) = (
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
                parts.next().unwrap_or(""),
            );
            all.push(format!("{ts} {stream} [{s}] {rest}"));
        }
    }
    all.sort();
    let skip = all.len().saturating_sub(n);
    all.split_off(skip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_lines() {
        let data = format!("short\n{}\nend", "x".repeat(MAX_LINE + 10));
        let mut r = BufReader::new(data.as_bytes());
        let mut buf = Vec::new();
        assert_eq!(read_line_bounded(&mut r, &mut buf).unwrap(), 6);
        buf.clear();
        assert_eq!(read_line_bounded(&mut r, &mut buf).unwrap(), MAX_LINE);
        buf.clear();
        assert_eq!(read_line_bounded(&mut r, &mut buf).unwrap(), 11);
        buf.clear();
        assert_eq!(read_line_bounded(&mut r, &mut buf).unwrap(), 3);
        buf.clear();
        assert_eq!(read_line_bounded(&mut r, &mut buf).unwrap(), 0);
    }

    #[test]
    fn rotate_and_tail() {
        let dir = std::env::temp_dir().join(format!("nexus-logs-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("svc.log");
        fs::write(numbered(&p, 1), "a\nb\n").unwrap();
        fs::write(&p, "c\nd\n").unwrap();
        assert_eq!(tail_file(&p, 3), vec!["b", "c", "d"]);
        assert_eq!(tail_file(&p, 10), vec!["a", "b", "c", "d"]);
        rotate(&p);
        assert!(!p.exists());
        assert_eq!(fs::read_to_string(numbered(&p, 1)).unwrap(), "c\nd\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
