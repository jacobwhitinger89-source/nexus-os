//! hello-http — a minimal NEXUS example application.
//!
//! * listens on $PORT (default 8080)
//! * GET /health -> 200 "ok" (used by the NEXUS health check)
//! * GET /       -> greeting with the release id
//! * GET /crash  -> exits with status 3 (demonstrates restart + diagnostics)
//! * publishes request counters to $NEXUS_METRICS_FILE (NEXUS app metrics)

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU64, Ordering};

static REQUESTS: AtomicU64 = AtomicU64::new(0);
static ERRORS: AtomicU64 = AtomicU64::new(0);

fn publish_metrics() {
    let Ok(path) = std::env::var("NEXUS_METRICS_FILE") else {
        return;
    };
    let body = format!(
        "{{\"metrics\":{{\"http_requests\":{},\"errors\":{}}}}}",
        REQUESTS.load(Ordering::Relaxed),
        ERRORS.load(Ordering::Relaxed)
    );
    // Write-then-rename so readers never see a partial file.
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let release = std::env::var("NEXUS_RELEASE").unwrap_or_else(|_| "local".into());
    let listener = match TcpListener::bind(format!("0.0.0.0:{port}")) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("hello-http: cannot listen on port {port}: {e}");
            std::process::exit(1);
        }
    };
    println!("hello-http listening on port {port} (release {release})");
    publish_metrics();
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        let mut first = String::new();
        let mut r = BufReader::new(match s.try_clone() {
            Ok(c) => c,
            Err(_) => continue,
        });
        if r.read_line(&mut first).is_err() {
            continue;
        }
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" || h == "\n" {
                break;
            }
        }
        REQUESTS.fetch_add(1, Ordering::Relaxed);
        let path = first.split_whitespace().nth(1).unwrap_or("/");
        let (status, body) = match path {
            "/health" => ("200 OK", "ok\n".to_string()),
            "/" => ("200 OK", format!("Hello from NEXUS OS (release {release})\n")),
            "/crash" => {
                eprintln!("hello-http: crash requested via /crash");
                std::process::exit(3);
            }
            _ => {
                ERRORS.fetch_add(1, Ordering::Relaxed);
                ("404 Not Found", "not found\n".to_string())
            }
        };
        let _ = write!(
            s,
            "HTTP/1.0 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        publish_metrics();
    }
}
