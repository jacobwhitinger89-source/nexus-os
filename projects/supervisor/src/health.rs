//! Health checks: HTTP GET, TCP connect, or a command run as the project
//! user. A dedicated thread picks due checks and runs each on a short-lived
//! worker thread so a slow check never stalls supervision.

use crate::state::{self, Ctx, ProjectPaths};
use nexus_core::manifest::HealthCheck;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub fn tcp(port: u16, timeout: Duration) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, timeout)
        .map(|_| ())
        .map_err(|e| format!("tcp 127.0.0.1:{port}: {e}"))
}

/// Minimal HTTP/1.0 GET; 2xx and 3xx are healthy.
pub fn http(port: u16, path: &str, timeout: Duration) -> Result<(), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("http 127.0.0.1:{port}{path}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(timeout)).ok();
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: nexus-health\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("http write: {e}"))?;
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).map_err(|e| format!("http read: {e}"))?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let code: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("http 127.0.0.1:{port}{path}: malformed response"))?;
    if (200..400).contains(&code) {
        Ok(())
    } else {
        Err(format!("http 127.0.0.1:{port}{path}: status {code}"))
    }
}

struct Due {
    project: String,
    service: String,
    pid: i32,
    check: HealthCheck,
    port: Option<u16>,
    cmd: Option<Command>,
}

fn run(due: Due) -> (bool, String) {
    let t = Duration::from_secs(due.check.timeout.max(1));
    let r = if let Some(path) = &due.check.http {
        http(due.port.unwrap_or(0), path, t)
    } else if let Some(port) = due.check.tcp {
        tcp(port, t)
    } else if let Some(mut cmd) = due.cmd {
        match cmd.spawn() {
            Ok(mut child) => {
                let start = Instant::now();
                loop {
                    match child.try_wait() {
                        Ok(Some(st)) if st.success() => break Ok(()),
                        Ok(Some(st)) => break Err(format!("command exited with {st}")),
                        Ok(None) if start.elapsed() > t => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break Err("command timed out".into());
                        }
                        Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                        Err(e) => break Err(e.to_string()),
                    }
                }
            }
            Err(e) => Err(format!("cannot run health command: {e}")),
        }
    } else {
        Ok(())
    };
    match r {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e),
    }
}

pub fn thread(ctx: Ctx) {
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let mut due: Vec<Due> = Vec::new();
        {
            let mut d = ctx.lock();
            if d.shutting_down {
                return;
            }
            let now = Instant::now();
            let names: Vec<String> = d.projects.keys().cloned().collect();
            for pn in names {
                let svc_names: Vec<String> = d.projects[&pn].services.keys().cloned().collect();
                for sn in svc_names {
                    let (policy, release) = {
                        let p = &d.projects[&pn];
                        (p.policy.clone(), p.release.clone())
                    };
                    let s = &d.projects[&pn].services[&sn];
                    let (Some(h), Some(pid)) = (s.spec.health.clone(), s.pid) else {
                        continue;
                    };
                    if s.health_running
                        || s.stop_deadline.is_some()
                        || s.next_health.is_some_and(|t| now < t)
                    {
                        continue;
                    }
                    let port = h.port.or(s.spec.ports.first().map(|p| p.port()));
                    let cmd = h.command.as_ref().and_then(|c| {
                        let rel = ProjectPaths::new(&pn).release(&release.as_ref()?.id);
                        let mut cmd = Command::new("/bin/sh");
                        cmd.arg("-c")
                            .arg(c)
                            .current_dir(&rel)
                            .env_clear()
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null());
                        for (k, v) in state::base_env(&policy, &rel) {
                            cmd.env(k, v);
                        }
                        let sb = state::sandbox_for(&d, &policy, "health");
                        crate::sandbox::apply(&mut cmd, &sb);
                        Some(cmd)
                    });
                    let s = d
                        .projects
                        .get_mut(&pn)
                        .unwrap()
                        .services
                        .get_mut(&sn)
                        .unwrap();
                    s.health_running = true;
                    due.push(Due {
                        project: pn.clone(),
                        service: sn.clone(),
                        pid,
                        check: h,
                        port,
                        cmd,
                    });
                }
            }
        }
        for item in due {
            let ctx = ctx.clone();
            let _ = std::thread::Builder::new()
                .stack_size(128 * 1024)
                .spawn(move || {
                    let (project, service, pid) =
                        (item.project.clone(), item.service.clone(), item.pid);
                    let (ok, detail) = run(item);
                    let mut d = ctx.lock();
                    state::health_result(&mut d, &project, &service, pid, ok, detail);
                    drop(d);
                    ctx.poke();
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn tcp_and_http_checks() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (i, s) in l.incoming().enumerate() {
                let mut s = s.unwrap();
                // Read the whole request so closing does not send a RST.
                let mut req = Vec::new();
                let mut buf = [0u8; 512];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let reply = if i == 1 {
                    "HTTP/1.0 503 Unavailable\r\n\r\n"
                } else {
                    "HTTP/1.0 200 OK\r\n\r\nok"
                };
                let _ = s.write_all(reply.as_bytes());
            }
        });
        let t = Duration::from_secs(2);
        assert!(http(port, "/health", t).is_ok());
        assert!(http(port, "/health", t).unwrap_err().contains("503"));
        assert!(tcp(port, t).is_ok());
        // Port 1 is never listening for an unprivileged test.
        assert!(tcp(1, t).is_err());
    }
}
