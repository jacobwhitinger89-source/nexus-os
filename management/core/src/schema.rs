//! Versioned machine-readable output (documented in
//! documentation/json-schema.md). Every top-level JSON document carries
//! `schema_version`; fields are only ever added within a major version.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Not running and not requested to run.
    Stopped,
    /// Waiting for dependencies to become ready.
    Waiting,
    /// Process spawned, health not yet confirmed.
    Starting,
    Running,
    Stopping,
    /// Exited unexpectedly; restart scheduled.
    Backoff,
    /// Gave up (crash loop, spawn error, dependency failure).
    Failed,
    /// Exited with status 0 and restart policy does not restart it.
    Exited,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// No health check configured.
    None,
    /// Within the start period, no verdict yet.
    Starting,
    Healthy,
    Unhealthy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    /// All services running (and healthy where checked).
    Active,
    /// Some services running, others failing or unhealthy.
    Degraded,
    /// Nothing running by request.
    Inactive,
    /// Services requested to run but none are running.
    Failed,
    /// A deployment is in progress.
    Deploying,
    /// The project exists but no release has been deployed.
    NotDeployed,
}

impl ProjectState {
    pub fn label(&self) -> &'static str {
        match self {
            ProjectState::Active => "ACTIVE",
            ProjectState::Degraded => "DEGRADED",
            ProjectState::Inactive => "INACTIVE",
            ProjectState::Failed => "FAILED",
            ProjectState::Deploying => "DEPLOYING",
            ProjectState::NotDeployed => "NOT DEPLOYED",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecoveryAction {
    /// Stable identifier, e.g. `restart`, `rollback`, `view_logs`.
    pub action: String,
    /// Exact CLI invocation that performs it.
    pub command: String,
    /// Permission required (see documentation/security.md).
    pub permission: String,
}

/// A structured failure report. `code` values are stable identifiers
/// listed in `codes` below.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub project: String,
    pub service: Option<String>,
    pub message: String,
    pub timestamp: String,
    #[serde(default)]
    pub details: Value,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
    #[serde(default)]
    pub recovery_actions: Vec<RecoveryAction>,
}

/// Stable diagnostic codes.
pub mod codes {
    pub const PORT_IN_USE: &str = "PORT_IN_USE";
    pub const EXEC_NOT_FOUND: &str = "EXEC_NOT_FOUND";
    pub const PERMISSION_DENIED: &str = "PERMISSION_DENIED";
    pub const OUT_OF_MEMORY: &str = "OUT_OF_MEMORY";
    pub const CRASH_LOOP: &str = "CRASH_LOOP";
    pub const EXITED_NONZERO: &str = "EXITED_NONZERO";
    pub const KILLED_BY_SIGNAL: &str = "KILLED_BY_SIGNAL";
    pub const HEALTH_CHECK_FAILED: &str = "HEALTH_CHECK_FAILED";
    pub const DEPENDENCY_FAILED: &str = "DEPENDENCY_FAILED";
    pub const DISK_FULL: &str = "DISK_FULL";
    pub const MEMORY_PRESSURE: &str = "MEMORY_PRESSURE";
    pub const PIDS_LIMIT: &str = "PIDS_LIMIT";
    pub const BUILD_FAILED: &str = "BUILD_FAILED";
    pub const DEPLOY_HEALTH_FAILED: &str = "DEPLOY_HEALTH_FAILED";
    pub const NO_RELEASE: &str = "NO_RELEASE";
    pub const MANIFEST_INVALID: &str = "MANIFEST_INVALID";
    pub const SPAWN_FAILED: &str = "SPAWN_FAILED";
    pub const UPLOAD_FAILED: &str = "UPLOAD_FAILED";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PortStatus {
    pub port: u16,
    pub protocol: String,
    pub public: bool,
    /// Whether something is listening on the port right now.
    pub listening: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceStatus {
    pub name: String,
    pub state: ServiceState,
    pub health: Health,
    pub pid: Option<u32>,
    pub uptime_seconds: Option<u64>,
    pub restarts: u32,
    pub last_exit: Option<ExitInfo>,
    /// Bytes, from the service cgroup (null when cgroups are unavailable).
    pub memory_bytes: Option<u64>,
    pub ports: Vec<PortStatus>,
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseInfo {
    pub id: String,
    pub deployed_at: String,
    pub deployed_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectResources {
    pub memory_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
    /// CPU utilisation over the last sampling window, percent of one core.
    pub cpu_percent: Option<f64>,
    pub pids: Option<u64>,
    pub oom_kills: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectStatus {
    pub schema_version: u32,
    pub project: String,
    pub id: String,
    pub status: ProjectState,
    pub release: Option<ReleaseInfo>,
    pub services: Vec<ServiceStatus>,
    /// Most relevant current failure, if any.
    pub failure: Option<Diagnostic>,
    pub resources: ProjectResources,
    /// Application-reported metrics (see documentation/app-integration.md);
    /// null when the application does not publish any.
    pub metrics: Option<Value>,
    pub diagnostics_available: bool,
    pub logs_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSummary {
    pub name: String,
    pub id: String,
    pub status: ProjectState,
    pub services_total: usize,
    pub services_running: usize,
    pub release: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeployState {
    Uploading,
    Building,
    Activating,
    Succeeded,
    Failed,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeploymentRecord {
    pub id: String,
    pub project: String,
    pub status: DeployState,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub deployed_by: String,
    pub message: Option<String>,
    pub previous_release: Option<String>,
    pub source_bytes: u64,
    pub error: Option<Diagnostic>,
}

/// Wrap a payload with the schema version and a kind tag.
pub fn envelope(kind: &str, data: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("schema_version".into(), SCHEMA_VERSION.into());
    obj.insert("kind".into(), kind.into());
    obj.insert("generated_at".into(), crate::time::now_rfc3339().into());
    match data {
        Value::Object(m) => obj.extend(m),
        other => {
            obj.insert("data".into(), other);
        }
    }
    Value::Object(obj)
}
