//! `nexus` — NEXUS OS command line interface and dashboard.
//!
//! Every command has stable `--json` output (documentation/cli.md,
//! documentation/json-schema.md). Exit codes: 0 success, 1 operation failed,
//! 2 usage error, 3 permission denied, 4 not found, 5 supervisor unreachable.

mod args;
mod dashboard;
mod render;
mod system;

use args::Args;
use nexus_core::ipc::{self, ErrorBody, Request};
use nexus_core::resources;
use nexus_core::schema::{self, DeploymentRecord, ProjectStatus, ProjectSummary};
use nexus_core::term;
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
NEXUS OS command line

USAGE: nexus <command> [arguments] [--json]

Server
  status                         overall server status
  resources [--health]           CPU, memory, storage, network, GPU
  dashboard                      interactive terminal dashboard

Projects
  projects                       list projects
  project status <p>             project, services, resources, failures
  project create <p> [--memory MiB] [--cpu PCT] [--pids N]     (admin)
  project limits <p> [--memory MiB] [--cpu PCT] [--pids N]     (admin)
  project delete <p> --confirm <p>                             (admin)
  start|stop|restart <p> [--service S]
  logs <p> [--service S] [--errors] [--lines N]
  doctor <p>                     run diagnostics
  deploy <p> [--source DIR|-] [--message TEXT]
                                 upload, build, activate, verify (rolls back
                                 automatically); default source is stdin:
                                 tar -czf - . | ssh host nexus deploy <p>
  history <p>                    deployment history
  rollback <p> [--to RELEASE]
  env list|set|unset <p> [KEY[=VALUE]]   server-side environment (secrets)

Access control (admin)
  agent add <name> --key 'ssh-ed25519 AAAA...' | --key-file FILE
  agent remove <name> --confirm <name>
  agent list
  access grant <user> <project|*> <perm,...>   perms: read,logs,upload,
  access revoke <user> <project>               build,deploy,restart,config,all
  access list
  audit [--lines N]
  whoami

SSH
  ssh status                     password policy and host key fingerprints
  ssh add-key 'ssh-ed25519 ...'  add a public key to your own account
  ssh finalize                   disable password logins (after a key login)
  ssh allow-passwords            re-enable password logins (admin)

System (root, e.g. via sudo)
  system hostname <name>
  system network dhcp | static --address A/P --gateway G [--dns 'D1 D2']
  system update <bundle.tar>     write update to the inactive A/B slot
  system slot | confirm-boot | reboot | poweroff

Add --json to any command for machine-readable output.";

#[derive(Debug)]
enum Fail {
    Usage(String),
    Remote(ErrorBody),
    Local(String),
    Unreachable(String),
}

impl From<io::Error> for Fail {
    fn from(e: io::Error) -> Self {
        Fail::Local(e.to_string())
    }
}

type R = Result<(), Fail>;

fn exit_code(f: &Fail) -> u8 {
    match f {
        Fail::Usage(_) => 2,
        Fail::Remote(e) if e.code == "FORBIDDEN" => 3,
        Fail::Remote(e) if e.code == "NOT_FOUND" => 4,
        Fail::Unreachable(_) => 5,
        _ => 1,
    }
}

fn print_json(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

fn call(req: Request) -> Result<Value, Fail> {
    call_with(req, None, |_, _| {})
}

fn call_with(
    req: Request,
    upload: Option<&mut dyn Read>,
    on_log: impl FnMut(&str, &str),
) -> Result<Value, Fail> {
    match ipc::call(&req, upload, on_log) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(Fail::Remote(e)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(Fail::Unreachable(e.to_string()))
        }
        Err(e) => Err(Fail::Local(e.to_string())),
    }
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let a = match Args::parse(raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("nexus: {e}");
            return ExitCode::from(2);
        }
    };
    let json = a.has("json");
    match run(&a) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            if json {
                let (code, message, details) = match &f {
                    Fail::Remote(e) => (e.code.clone(), e.message.clone(), e.details.clone()),
                    Fail::Usage(m) => ("USAGE".into(), m.clone(), Value::Null),
                    Fail::Local(m) => ("FAILED".into(), m.clone(), Value::Null),
                    Fail::Unreachable(m) => {
                        ("SUPERVISOR_UNREACHABLE".into(), m.clone(), Value::Null)
                    }
                };
                print_json(&schema::envelope(
                    "error",
                    json!({ "error": { "code": code, "message": message, "details": details } }),
                ));
            } else {
                match &f {
                    Fail::Usage(m) => eprintln!("nexus: {m}\nRun 'nexus help' for usage."),
                    Fail::Remote(e) => {
                        eprintln!("nexus: {} ({})", e.message, e.code);
                        if let Some(d) = e.details.get("deployment").and_then(|d| d.get("error"))
                            && let Ok(diag) =
                                serde_json::from_value::<schema::Diagnostic>(d.clone())
                        {
                            let mut o = String::new();
                            render::diagnostic(&mut o, &diag, true);
                            eprint!("{o}");
                        }
                    }
                    Fail::Local(m) | Fail::Unreachable(m) => eprintln!("nexus: {m}"),
                }
            }
            ExitCode::from(exit_code(&f))
        }
    }
}

fn run(a: &Args) -> R {
    let json = a.has("json");
    let cmd = a.word(0).unwrap_or(if term::is_tty(0) && term::is_tty(1) {
        "dashboard"
    } else {
        "help"
    });
    if a.has("help") && cmd != "help" {
        println!("{HELP}");
        return Ok(());
    }
    match cmd {
        "help" => {
            println!("{HELP}");
            Ok(())
        }
        "version" | "--version" => {
            if json {
                print_json(
                    &json!({ "schema_version": schema::SCHEMA_VERSION, "version": nexus_core::VERSION, "os_version": resources::os_version() }),
                );
            } else {
                println!(
                    "nexus {} (NEXUS OS {})",
                    nexus_core::VERSION,
                    resources::os_version()
                );
            }
            Ok(())
        }
        "status" => cmd_status(json),
        "resources" => {
            let (r, _) = resources::collect(None, Duration::from_millis(500), a.has("health"));
            if json {
                print_json(&serde_json::to_value(&r).unwrap_or_default());
            } else {
                print!("{}", render::resources_full(&r));
            }
            Ok(())
        }
        "dashboard" => dashboard::run().map_err(Fail::from),
        "projects" => {
            let v = call(Request::Projects)?;
            if json {
                print_json(&v);
            } else {
                let list: Vec<ProjectSummary> =
                    serde_json::from_value(v["projects"].clone()).unwrap_or_default();
                print!("{}", render::projects_table(&list));
            }
            Ok(())
        }
        "project" => cmd_project(a, json),
        "start" | "stop" | "restart" => {
            let project = a.need(1, "project name").map_err(Fail::Usage)?.to_string();
            let service = a.get("service").map(String::from);
            let req = match cmd {
                "start" => Request::Start {
                    project: project.clone(),
                    service,
                },
                "stop" => Request::Stop {
                    project: project.clone(),
                    service,
                },
                _ => Request::Restart {
                    project: project.clone(),
                    service,
                },
            };
            let v = call(req)?;
            if json {
                print_json(&schema::envelope("action", v));
            } else {
                println!(
                    "{cmd} requested for {project}; check with: nexus project status {project}"
                );
            }
            Ok(())
        }
        "logs" => {
            let project = a.need(1, "project name").map_err(Fail::Usage)?.to_string();
            let lines = a.num("lines").map_err(Fail::Usage)?.unwrap_or(100) as usize;
            let v = call(Request::Logs {
                project,
                service: a.get("service").map(String::from),
                errors: a.has("errors"),
                lines,
            })?;
            if json {
                print_json(&v);
            } else {
                let lines = v["lines"].as_array().cloned().unwrap_or_default();
                if lines.is_empty() {
                    println!("(no log lines)");
                }
                for l in lines {
                    println!("{}", l.as_str().unwrap_or(""));
                }
            }
            Ok(())
        }
        "doctor" => {
            let project = a.need(1, "project name").map_err(Fail::Usage)?.to_string();
            let v = call(Request::Doctor { project })?;
            if json {
                print_json(&v);
            } else {
                print!("{}", render::doctor(&v));
            }
            Ok(())
        }
        "deploy" => cmd_deploy(a, json),
        "history" => {
            let project = a.need(1, "project name").map_err(Fail::Usage)?.to_string();
            let v = call(Request::History { project })?;
            if json {
                print_json(&v);
            } else {
                let list: Vec<DeploymentRecord> =
                    serde_json::from_value(v["deployments"].clone()).unwrap_or_default();
                print!("{}", render::history(&list));
            }
            Ok(())
        }
        "rollback" => {
            let project = a.need(1, "project name").map_err(Fail::Usage)?.to_string();
            let v = call(Request::Rollback {
                project,
                to: a.get("to").map(String::from),
            })?;
            if json {
                print_json(&schema::envelope("rollback", v));
            } else {
                println!(
                    "rolled back to {}",
                    v["rolled_back_to"].as_str().unwrap_or("?")
                );
            }
            Ok(())
        }
        "env" => cmd_env(a, json),
        "access" => cmd_access(a, json),
        "agent" => cmd_agent(a, json),
        "audit" => {
            let lines = a.num("lines").map_err(Fail::Usage)?.unwrap_or(50) as usize;
            let v = call(Request::Audit { lines })?;
            if json {
                print_json(&v);
            } else {
                for e in v["entries"].as_array().into_iter().flatten() {
                    println!(
                        "{} {:<10} {:<18} {:<14} {}",
                        e["ts"].as_str().unwrap_or(""),
                        e["user"].as_str().unwrap_or(""),
                        e["action"].as_str().unwrap_or(""),
                        e["project"].as_str().unwrap_or("-"),
                        e["result"].as_str().unwrap_or("")
                    );
                }
            }
            Ok(())
        }
        "whoami" => {
            let v = call(Request::Whoami)?;
            if json {
                print_json(&schema::envelope("whoami", v));
            } else {
                println!(
                    "{} (uid {}){}",
                    v["user"].as_str().unwrap_or("?"),
                    v["uid"],
                    if v["admin"] == true {
                        " — administrator"
                    } else {
                        ""
                    }
                );
                if let Some(g) = v["grants"].as_object() {
                    for (p, perms) in g {
                        println!(
                            "  {p}: {}",
                            perms
                                .as_array()
                                .map(|x| x
                                    .iter()
                                    .filter_map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(","))
                                .unwrap_or_default()
                        );
                    }
                }
            }
            Ok(())
        }
        "ssh" => cmd_ssh(a, json),
        "system" => system::run(a, json),
        "agent-exec" => agent_exec(),
        other => Err(Fail::Usage(format!("unknown command {other:?}"))),
    }
}

fn cmd_status(json: bool) -> R {
    let (res, _) = resources::collect(None, Duration::from_millis(300), false);
    let daemon = call(Request::Status);
    if json {
        let mut v = match &daemon {
            Ok(v) => v.clone(),
            Err(_) => schema::envelope("status", json!({})),
        };
        v["status"] = json!(if daemon.is_ok() { "online" } else { "degraded" });
        v["supervisor_reachable"] = json!(daemon.is_ok());
        v["resources"] = serde_json::to_value(&res).unwrap_or_default();
        print_json(&v);
        return Ok(());
    }
    let mut o = String::new();
    render::header(&mut o, "NEXUS OS");
    o.push_str(&format!(
        "\n HOSTNAME: {}\n STATUS:   {}\n UPTIME:   {}\n VERSION:  {}\n",
        res.hostname,
        if daemon.is_ok() {
            "ONLINE"
        } else {
            "DEGRADED (supervisor not reachable)"
        },
        nexus_core::time::human_duration(res.uptime_seconds),
        res.os_version
    ));
    render::resources_brief(&mut o, &res);
    render::section(&mut o, "PROJECTS");
    match daemon {
        Ok(v) => {
            let list: Vec<ProjectSummary> =
                serde_json::from_value(v["projects"].clone()).unwrap_or_default();
            o.push_str(&render::projects_table(&list));
            if v["supervisor"]["isolation"] == "limited" {
                o.push_str("\n NOTE: supervisor isolation is LIMITED (no cgroups or not root)\n");
            }
            if v["ssh"]["password_auth"] == true {
                o.push_str(" NOTE: SSH password logins are enabled; run 'nexus ssh finalize' after key setup\n");
            }
        }
        Err(e) => o.push_str(&format!(" unavailable: {e:?}\n")),
    }
    print!("{o}");
    Ok(())
}

fn cmd_project(a: &Args, json: bool) -> R {
    let sub = a.word(1).ok_or_else(|| {
        Fail::Usage("project needs a subcommand (status, create, limits, delete)".into())
    })?;
    let project = a.need(2, "project name").map_err(Fail::Usage)?.to_string();
    type Limits = (Option<u64>, Option<u64>, Option<u64>);
    let limits = || -> Result<Limits, Fail> {
        Ok((
            a.num("memory").map_err(Fail::Usage)?,
            a.num("cpu").map_err(Fail::Usage)?,
            a.num("pids").map_err(Fail::Usage)?,
        ))
    };
    let v = match sub {
        "status" => {
            let v = call(Request::ProjectStatus { project })?;
            if json {
                print_json(&v);
            } else {
                let s: ProjectStatus =
                    serde_json::from_value(v).map_err(|e| Fail::Local(e.to_string()))?;
                print!("{}", render::project_status(&s));
            }
            return Ok(());
        }
        "create" => {
            let (memory_mb, cpu_percent, pids) = limits()?;
            call(Request::ProjectCreate {
                project,
                memory_mb,
                cpu_percent,
                pids,
            })?
        }
        "limits" => {
            let (memory_mb, cpu_percent, pids) = limits()?;
            call(Request::LimitsSet {
                project,
                memory_mb,
                cpu_percent,
                pids,
            })?
        }
        "delete" => {
            let confirm = a.get("confirm").unwrap_or("").to_string();
            call(Request::ProjectDelete { project, confirm })?
        }
        other => return Err(Fail::Usage(format!("unknown project subcommand {other:?}"))),
    };
    if json {
        print_json(&schema::envelope(&format!("project.{sub}"), v));
    } else {
        match sub {
            "create" => println!(
                "project {} created (service user {}, memory limit {} MiB)\nnext: {}",
                v["project"].as_str().unwrap_or(""),
                v["user"].as_str().unwrap_or(""),
                v["limits"]["memory_mb"],
                v["next"].as_str().unwrap_or("")
            ),
            _ => println!("{sub}: ok"),
        }
    }
    Ok(())
}

/// Wraps stdin so the first bytes (used for compression sniffing) are
/// replayed before the rest.
struct Prefixed<R: Read> {
    head: Vec<u8>,
    pos: usize,
    rest: R,
}

impl<R: Read> Read for Prefixed<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.head.len() {
            let n = (self.head.len() - self.pos).min(buf.len());
            buf[..n].copy_from_slice(&self.head[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.rest.read(buf)
    }
}

fn cmd_deploy(a: &Args, json: bool) -> R {
    let project = a.need(1, "project name").map_err(Fail::Usage)?.to_string();
    let source = a.get("source").unwrap_or("-");
    let message = a.get("message").map(String::from);
    let mut on_log = |stream: &str, line: &str| {
        if !json {
            if stream == "build" {
                eprintln!("  | {line}");
            } else {
                eprintln!("==> {line}");
            }
        }
    };
    let v = if source == "-" {
        if term::is_tty(0) {
            return Err(Fail::Usage(
                "deploy reads a tar archive on stdin, e.g.: tar -czf - . | ssh <host> nexus deploy <project> (or use --source DIR on the server)".into(),
            ));
        }
        let mut stdin = io::stdin().lock();
        let mut head = vec![0u8; 2];
        let mut got = 0;
        while got < 2 {
            let n = stdin.read(&mut head[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        head.truncate(got);
        let compression = if head == [0x1f, 0x8b] { "gzip" } else { "none" };
        let mut src = Prefixed {
            head,
            pos: 0,
            rest: stdin,
        };
        call_with(
            Request::Deploy {
                project,
                compression: compression.into(),
                message,
            },
            Some(&mut src),
            &mut on_log,
        )?
    } else {
        let mut child = std::process::Command::new("tar")
            .args(["-czf", "-", "-C", source, "."])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Fail::Local(format!("cannot run tar: {e}")))?;
        let mut out = child.stdout.take().unwrap();
        let r = call_with(
            Request::Deploy {
                project,
                compression: "gzip".into(),
                message,
            },
            Some(&mut out),
            &mut on_log,
        );
        let st = child.wait()?;
        if !st.success() {
            return Err(Fail::Local(format!("tar of {source} failed")));
        }
        r?
    };
    if json {
        print_json(&schema::envelope("deploy", v));
    } else {
        println!("deployed {}", v["deployment"]["id"].as_str().unwrap_or("?"));
    }
    Ok(())
}

fn cmd_env(a: &Args, json: bool) -> R {
    let sub = a
        .word(1)
        .ok_or_else(|| Fail::Usage("env needs list, set or unset".into()))?;
    let project = a.need(2, "project name").map_err(Fail::Usage)?.to_string();
    let v = match sub {
        "list" => call(Request::EnvList { project })?,
        "set" => {
            let kv = a.need(3, "KEY=VALUE").map_err(Fail::Usage)?;
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| Fail::Usage("expected KEY=VALUE".into()))?;
            call(Request::EnvSet {
                project,
                key: k.into(),
                value: v.into(),
            })?
        }
        "unset" => call(Request::EnvUnset {
            project,
            key: a.need(3, "KEY").map_err(Fail::Usage)?.into(),
        })?,
        other => return Err(Fail::Usage(format!("unknown env subcommand {other:?}"))),
    };
    if json {
        print_json(&schema::envelope(&format!("env.{sub}"), v));
    } else if sub == "list" {
        for k in v["keys"].as_array().into_iter().flatten() {
            println!("{}", k.as_str().unwrap_or(""));
        }
    } else {
        println!("ok — restart the project to apply");
    }
    Ok(())
}

fn cmd_access(a: &Args, json: bool) -> R {
    let sub = a
        .word(1)
        .ok_or_else(|| Fail::Usage("access needs grant, revoke or list".into()))?;
    let v = match sub {
        "grant" => {
            let user = a.need(2, "user").map_err(Fail::Usage)?.to_string();
            let project = a.need(3, "project").map_err(Fail::Usage)?.to_string();
            let perms = a.need(4, "permissions").map_err(Fail::Usage)?;
            call(Request::AccessGrant {
                user,
                project,
                permissions: perms.split(',').map(String::from).collect(),
            })?
        }
        "revoke" => call(Request::AccessRevoke {
            user: a.need(2, "user").map_err(Fail::Usage)?.into(),
            project: a.need(3, "project").map_err(Fail::Usage)?.into(),
        })?,
        "list" => call(Request::AccessList)?,
        other => return Err(Fail::Usage(format!("unknown access subcommand {other:?}"))),
    };
    if json {
        print_json(&schema::envelope(&format!("access.{sub}"), v));
    } else if sub == "list" {
        for (user, projects) in v["grants"].as_object().into_iter().flatten() {
            for (p, perms) in projects.as_object().into_iter().flatten() {
                let list: Vec<&str> = perms
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|x| x.as_str())
                    .collect();
                println!("{user:<20} {p:<28} {}", list.join(","));
            }
        }
    } else {
        println!("ok");
    }
    Ok(())
}

fn cmd_agent(a: &Args, json: bool) -> R {
    let sub = a
        .word(1)
        .ok_or_else(|| Fail::Usage("agent needs add, remove or list".into()))?;
    let v = match sub {
        "add" => {
            let name = a.need(2, "agent name").map_err(Fail::Usage)?.to_string();
            let key = match (a.get("key"), a.get("key-file")) {
                (Some(k), _) => k.to_string(),
                (None, Some(f)) => std::fs::read_to_string(f)?,
                _ => return Err(Fail::Usage("agent add needs --key or --key-file".into())),
            };
            call(Request::AgentCreate {
                name,
                public_key: key,
            })?
        }
        "remove" => {
            let name = a.need(2, "agent name").map_err(Fail::Usage)?.to_string();
            call(Request::AgentRemove {
                name,
                confirm: a.get("confirm").unwrap_or("").into(),
            })?
        }
        "list" => call(Request::AgentList)?,
        other => return Err(Fail::Usage(format!("unknown agent subcommand {other:?}"))),
    };
    if json {
        print_json(&schema::envelope(&format!("agent.{sub}"), v));
    } else {
        match sub {
            "add" => println!(
                "agent account {} created.\nnext: {}",
                v["user"].as_str().unwrap_or(""),
                v["next"].as_str().unwrap_or("")
            ),
            "list" => {
                for ag in v["agents"].as_array().into_iter().flatten() {
                    println!("{:<24} {}", ag["user"].as_str().unwrap_or(""), ag["grants"]);
                }
            }
            _ => println!("ok"),
        }
    }
    Ok(())
}

fn cmd_ssh(a: &Args, json: bool) -> R {
    let sub = a.word(1).unwrap_or("status");
    match sub {
        "add-key" => {
            let key = a.need(2, "public key").map_err(Fail::Usage)?;
            let t = key.split_whitespace().next().unwrap_or("");
            if !(t.starts_with("ssh-") || t.starts_with("ecdsa-") || t.starts_with("sk-"))
                || key.contains('\n')
            {
                return Err(Fail::Usage("expected one OpenSSH public key line".into()));
            }
            if key.contains("PRIVATE KEY") {
                return Err(Fail::Usage(
                    "that is a private key — never copy private keys to a server".into(),
                ));
            }
            let home = std::env::var("HOME").map_err(|_| Fail::Local("HOME not set".into()))?;
            let dir = std::path::Path::new(&home).join(".ssh");
            std::fs::create_dir_all(&dir)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            let path = dir.join("authorized_keys");
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !existing.lines().any(|l| l.trim() == key.trim()) {
                let mut f = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)?;
                writeln!(f, "{}", key.trim())?;
                // Survive a power cut: losing the key locks the admin out
                // once password logins are disabled.
                f.sync_all()?;
                nexus_core::fsutil::sync_parent(&path)?;
            }
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            if json {
                print_json(&schema::envelope(
                    "ssh.add_key",
                    json!({ "added": true, "file": path }),
                ));
            } else {
                println!(
                    "key added to {}.\nTest it from your computer:  ssh -o PasswordAuthentication=no <user>@<host> nexus ssh finalize",
                    path.display()
                );
            }
            Ok(())
        }
        "status" | "finalize" | "allow-passwords" => {
            let v = call(match sub {
                "status" => Request::SshStatus,
                "finalize" => Request::SshFinalize,
                _ => Request::SshAllowPasswords,
            })?;
            if json {
                print_json(&schema::envelope(&format!("ssh.{sub}"), v));
            } else if sub == "status" {
                println!(
                    "password logins: {}",
                    if v["password_auth"] == true {
                        "ENABLED (bootstrap)"
                    } else {
                        "disabled (keys only)"
                    }
                );
                for k in v["host_keys"].as_array().into_iter().flatten() {
                    println!(
                        "host key {:<8} {}",
                        k["type"].as_str().unwrap_or(""),
                        k["fingerprint"].as_str().unwrap_or("")
                    );
                }
            } else {
                println!("{}", v["note"].as_str().unwrap_or("ok"));
            }
            Ok(())
        }
        other => Err(Fail::Usage(format!("unknown ssh subcommand {other:?}"))),
    }
}

/// Forced command for agent SSH keys: only `nexus ...` invocations run.
fn agent_exec() -> R {
    let cmdline = std::env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let words = args::shell_words(&cmdline).map_err(Fail::Usage)?;
    let rest: Vec<String> = match words.first().map(String::as_str) {
        Some("nexus") | Some("/usr/bin/nexus") => words[1..].to_vec(),
        None => {
            println!(
                "{HELP}\n\nThis key may only run nexus commands, e.g.: ssh <host> nexus status --json"
            );
            return Ok(());
        }
        Some(other) => {
            return Err(Fail::Usage(format!(
                "this account may only run nexus commands (got {other:?})"
            )));
        }
    };
    if matches!(
        rest.first().map(String::as_str),
        Some("agent-exec") | Some("dashboard") | Some("system")
    ) {
        return Err(Fail::Usage("command not available to agent keys".into()));
    }
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("/usr/bin/nexus")
        .args(&rest)
        .exec();
    Err(Fail::Local(format!("exec failed: {err}")))
}
