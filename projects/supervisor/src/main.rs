//! nexusd — the NEXUS OS project supervisor.
//!
//! Runs as root under BusyBox init (respawned if it dies). Supervises every
//! project service, serves the control socket used by the `nexus` CLI and
//! performs deployments. See documentation/architecture.md.

mod accounts;
mod auth;
mod cgroup;
mod deploy;
mod diagnose;
mod firewall;
mod health;
mod logs;
mod sandbox;
mod seccomp;
mod server;
mod state;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn main() {
    if std::env::args().any(|a| a == "--version" || a == "-V") {
        println!("nexusd {}", nexus_core::VERSION);
        return;
    }
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::umask(0o022);
    }

    let cgroups = sandbox::is_root() && cgroup::setup();
    if cgroups {
        let n = cgroup::reap_orphans();
        if n > 0 {
            eprintln!("nexusd: killed {n} processes left by a previous instance");
        }
    } else {
        eprintln!("nexusd: cgroup v2 unavailable or not root — running with limited isolation");
    }
    let _ = fs::create_dir_all(nexus_core::paths::projects());
    let _ = fs::create_dir_all(nexus_core::paths::metrics_dir());

    let ctx = state::new_ctx(cgroups);
    state::load_all(&ctx);

    let sock = nexus_core::paths::socket();
    if let Some(dir) = sock.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::remove_file(&sock);
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("nexusd: cannot bind {}: {e}", sock.display());
            std::process::exit(1);
        }
    };
    // Every local user may connect; authorisation is per request (SO_PEERCRED).
    let _ = fs::set_permissions(&sock, fs::Permissions::from_mode(0o666));

    {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("monitor".into())
            .spawn(move || monitor(ctx))
            .expect("monitor thread");
    }
    {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("health".into())
            .spawn(move || health::thread(ctx))
            .expect("health thread");
    }
    {
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("accept".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    let Ok(conn) = conn else { continue };
                    let ctx = ctx.clone();
                    let _ = std::thread::Builder::new()
                        .stack_size(256 * 1024)
                        .spawn(move || server::handle(ctx, conn));
                }
            })
            .expect("accept thread");
    }
    eprintln!("nexusd {} ready on {}", nexus_core::VERSION, sock.display());

    while !STOP.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(200));
    }
    shutdown(&ctx);
    let _ = fs::remove_file(&sock);
}

fn monitor(ctx: state::Ctx) {
    let mut guard = ctx.lock();
    loop {
        state::tick(&mut guard);
        let desired = state::desired_ports(&guard);
        let firewall_changed = desired != guard.firewall_ports;
        drop(guard);
        if firewall_changed && sandbox::is_root() && firewall::apply(&desired) {
            ctx.lock().firewall_ports = desired;
        }
        guard = ctx.lock();
        let (g, _) = ctx
            .wake
            .wait_timeout(guard, Duration::from_millis(250))
            .unwrap_or_else(|e| e.into_inner());
        guard = g;
    }
}

/// Stop every project in dependency order, then return.
fn shutdown(ctx: &state::Ctx) {
    eprintln!("nexusd: stopping all projects");
    {
        let mut d = ctx.lock();
        d.shutting_down = true;
        let names: Vec<String> = d.projects.keys().cloned().collect();
        for n in names {
            // Keep the persistent desire to run (autostart on next boot):
            // only the live service state changes.
            let p = d.projects.get_mut(&n).unwrap();
            for s in p.services.values_mut() {
                s.want = false;
            }
        }
    }
    ctx.poke();
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        if ctx.lock().projects.values().all(|p| p.all_stopped()) {
            eprintln!("nexusd: all projects stopped");
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!("nexusd: shutdown timeout, killing remaining services");
    if ctx.lock().cgroups {
        cgroup::reap_orphans();
    }
}
