//! Project configuration.
//!
//! * `nexus.toml` — the application manifest, shipped inside every deployed
//!   release (services, build command, health checks). Rolling back a release
//!   rolls back its manifest too.
//! * `project.toml` — the server-side policy written by an administrator
//!   (identity, resource limits, secret environment). Application code can
//!   never change it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const MANIFEST_FILE: &str = "nexus.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub project: ProjectSection,
    #[serde(default)]
    pub build: Option<BuildSection>,
    #[serde(default)]
    pub deploy: DeploySection,
    #[serde(default, rename = "service")]
    pub services: Vec<Service>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectSection {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BuildSection {
    /// Shell command run as the project user in the release directory.
    pub command: String,
    /// Seconds before the build is killed.
    #[serde(default = "default_build_timeout")]
    pub timeout: u64,
}

fn default_build_timeout() -> u64 {
    1800
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeploySection {
    /// Seconds the new release has to become healthy before rollback.
    #[serde(default = "default_health_timeout")]
    pub health_timeout: u64,
    /// Number of old releases kept on disk (the current and the last
    /// successful one are always kept in addition).
    #[serde(default = "default_keep")]
    pub keep_releases: usize,
}

impl Default for DeploySection {
    fn default() -> Self {
        Self {
            health_timeout: default_health_timeout(),
            keep_releases: default_keep(),
        }
    }
}

fn default_health_timeout() -> u64 {
    60
}
fn default_keep() -> usize {
    3
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    Never,
    #[default]
    OnFailure,
    Always,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PortSpec {
    Port(u16),
    Detailed {
        port: u16,
        #[serde(default)]
        public: bool,
        #[serde(default = "default_proto")]
        protocol: String,
    },
}

fn default_proto() -> String {
    "tcp".into()
}

impl PortSpec {
    pub fn port(&self) -> u16 {
        match self {
            PortSpec::Port(p) => *p,
            PortSpec::Detailed { port, .. } => *port,
        }
    }
    /// Bare port numbers are public: listing a port in the manifest is the
    /// developer's request to serve it. Use `{ port = N, public = false }`
    /// for loopback-only services such as databases.
    pub fn public(&self) -> bool {
        match self {
            PortSpec::Port(_) => true,
            PortSpec::Detailed { public, .. } => *public,
        }
    }
    pub fn protocol(&self) -> &str {
        match self {
            PortSpec::Port(_) => "tcp",
            PortSpec::Detailed { protocol, .. } => protocol,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub name: String,
    /// Shell command, executed as `/bin/sh -c "exec <command>"`.
    pub command: String,
    /// Working directory relative to the release root.
    #[serde(default)]
    pub workdir: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub restart: RestartPolicy,
    /// Base restart delay in seconds (doubles per consecutive failure, max 60).
    #[serde(default = "default_restart_delay")]
    pub restart_delay: u64,
    /// Seconds between SIGTERM and SIGKILL on stop.
    #[serde(default = "default_stop_timeout")]
    pub stop_timeout: u64,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub ports: Vec<PortSpec>,
    #[serde(default)]
    pub health: Option<HealthCheck>,
}

fn default_restart_delay() -> u64 {
    1
}
fn default_stop_timeout() -> u64 {
    10
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HealthCheck {
    /// HTTP path probed with GET on 127.0.0.1:`port` (2xx/3xx = healthy).
    pub http: Option<String>,
    /// TCP port that must accept connections.
    pub tcp: Option<u16>,
    /// Port for `http`; defaults to the service's first port.
    pub port: Option<u16>,
    /// Command run as the project user; exit 0 = healthy.
    pub command: Option<String>,
    #[serde(default = "default_interval")]
    pub interval: u64,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
    #[serde(default = "default_retries")]
    pub retries: u32,
    /// Grace period after start before failures count.
    #[serde(default = "default_start_period")]
    pub start_period: u64,
}

fn default_interval() -> u64 {
    10
}
fn default_timeout() -> u64 {
    3
}
fn default_retries() -> u32 {
    3
}
fn default_start_period() -> u64 {
    30
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.services.is_empty() {
            return Err("nexus.toml defines no [[service]] entries".into());
        }
        let names: Vec<&str> = self.services.iter().map(|s| s.name.as_str()).collect();
        for (i, s) in self.services.iter().enumerate() {
            if !crate::valid_name(&s.name) {
                return Err(format!(
                    "service name {:?} is invalid (use lowercase letters, digits, '-')",
                    s.name
                ));
            }
            if names[..i].contains(&s.name.as_str()) {
                return Err(format!("service {:?} is defined twice", s.name));
            }
            if s.command.trim().is_empty() {
                return Err(format!("service {:?} has an empty command", s.name));
            }
            if let Some(w) = &s.workdir
                && (w.starts_with('/') || w.split('/').any(|c| c == ".."))
            {
                return Err(format!(
                    "service {:?}: workdir must be relative to the release",
                    s.name
                ));
            }
            for d in &s.depends_on {
                if !names.contains(&d.as_str()) {
                    return Err(format!(
                        "service {:?} depends on unknown service {d:?}",
                        s.name
                    ));
                }
            }
            for k in s.env.keys() {
                if k.is_empty() || k.starts_with("NEXUS_") || k.contains('=') {
                    return Err(format!("service {:?}: invalid env name {k:?}", s.name));
                }
            }
            if let Some(h) = &s.health {
                let kinds = [h.http.is_some(), h.tcp.is_some(), h.command.is_some()]
                    .iter()
                    .filter(|x| **x)
                    .count();
                if kinds != 1 {
                    return Err(format!(
                        "service {:?}: health check needs exactly one of http, tcp, command",
                        s.name
                    ));
                }
                if h.http.is_some() && h.port.or(s.ports.first().map(|p| p.port())).is_none() {
                    return Err(format!(
                        "service {:?}: http health check needs a port",
                        s.name
                    ));
                }
            }
        }
        self.start_order().map(|_| ())
    }

    /// Topological start order (dependencies first). Errors on cycles.
    pub fn start_order(&self) -> Result<Vec<String>, String> {
        let mut order: Vec<String> = Vec::new();
        let mut visiting: Vec<String> = Vec::new();
        fn visit(
            m: &Manifest,
            name: &str,
            order: &mut Vec<String>,
            visiting: &mut Vec<String>,
        ) -> Result<(), String> {
            if order.iter().any(|n| n == name) {
                return Ok(());
            }
            if visiting.iter().any(|n| n == name) {
                return Err(format!("dependency cycle involving service {name:?}"));
            }
            visiting.push(name.to_string());
            if let Some(s) = m.services.iter().find(|s| s.name == name) {
                for d in &s.depends_on {
                    visit(m, d, order, visiting)?;
                }
            }
            visiting.pop();
            order.push(name.to_string());
            Ok(())
        }
        for s in &self.services {
            visit(self, &s.name, &mut order, &mut visiting)?;
        }
        Ok(order)
    }

    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|s| s.name == name)
    }
}

/// Server-side project policy (`/data/nexus/projects/<name>/project.toml`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub id: String,
    pub name: String,
    pub user: String,
    pub uid: u32,
    pub created: String,
    #[serde(default = "yes")]
    pub autostart: bool,
    #[serde(default)]
    pub limits: Limits,
    /// Secret/server-specific environment; values are never shown in status.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    /// Memory limit for all services of the project (MiB). 0 = unlimited.
    pub memory_mb: u64,
    /// CPU limit in percent of one core (100 = one core). 0 = unlimited.
    pub cpu_percent: u64,
    /// Maximum number of tasks (processes + threads).
    pub pids: u64,
    /// Open file descriptors per process.
    pub nofile: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_mb: 512,
            cpu_percent: 0,
            pids: 512,
            nofile: 4096,
        }
    }
}

impl Policy {
    pub fn parse(text: &str) -> Result<Policy, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[project]
name = "flight-efb"

[build]
command = "cargo build --release"

[[service]]
name = "database"
command = "./db --port 5432"
ports = [{ port = 5432, public = false }]
health = { tcp = 5432 }

[[service]]
name = "api-server"
command = "./api"
depends_on = ["database"]
restart = "always"
ports = [3000]
env = { RUST_LOG = "info" }
[service.health]
http = "/health"
"#;

    #[test]
    fn parses_and_orders() {
        let m = Manifest::parse(SAMPLE).unwrap();
        assert_eq!(m.services.len(), 2);
        assert_eq!(m.start_order().unwrap(), vec!["database", "api-server"]);
        let api = m.service("api-server").unwrap();
        assert_eq!(api.restart, RestartPolicy::Always);
        assert!(api.ports[0].public());
        assert!(!m.service("database").unwrap().ports[0].public());
        assert_eq!(api.health.as_ref().unwrap().interval, 10);
    }

    #[test]
    fn rejects_bad_manifests() {
        assert!(Manifest::parse("").is_err());
        let cyc = r#"
[[service]]
name = "a"
command = "x"
depends_on = ["b"]
[[service]]
name = "b"
command = "y"
depends_on = ["a"]
"#;
        assert!(Manifest::parse(cyc).unwrap_err().contains("cycle"));
        let unknown = "[[service]]\nname = \"a\"\ncommand = \"x\"\nbogus = 1\n";
        assert!(Manifest::parse(unknown).is_err());
        let escape = "[[service]]\nname = \"a\"\ncommand = \"x\"\nworkdir = \"../..\"\n";
        assert!(Manifest::parse(escape).is_err());
        let reserved = "[[service]]\nname = \"a\"\ncommand = \"x\"\nenv = { NEXUS_X = \"1\" }\n";
        assert!(Manifest::parse(reserved).is_err());
        let two =
            "[[service]]\nname = \"a\"\ncommand = \"x\"\nhealth = { tcp = 1, command = \"y\" }\n";
        assert!(Manifest::parse(two).is_err());
    }

    #[test]
    fn policy_roundtrip() {
        let p = Policy {
            id: "prj-1".into(),
            name: "x".into(),
            user: "nx-x".into(),
            uid: 2001,
            created: "2026-01-01T00:00:00Z".into(),
            autostart: true,
            limits: Limits::default(),
            env: BTreeMap::new(),
        };
        assert_eq!(Policy::parse(&p.to_toml()).unwrap(), p);
    }
}
