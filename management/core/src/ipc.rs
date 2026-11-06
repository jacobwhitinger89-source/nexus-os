//! nexusd control protocol.
//!
//! Transport: the Unix stream socket `paths::socket()`. The client writes one
//! JSON `Request` line. For `Deploy` the raw source archive follows the
//! newline until the client shuts down its write side. The daemon answers
//! with JSON lines: zero or more `{"type":"log",...}` progress events and
//! exactly one final `{"type":"result",...}`.
//!
//! The daemon authenticates callers with SO_PEERCRED; no secrets travel
//! over the socket.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Whoami,
    Status,
    Projects,
    ProjectStatus {
        project: String,
    },
    ProjectCreate {
        project: String,
        #[serde(default)]
        memory_mb: Option<u64>,
        #[serde(default)]
        cpu_percent: Option<u64>,
        #[serde(default)]
        pids: Option<u64>,
    },
    /// Destructive: requires `confirm == project`.
    ProjectDelete {
        project: String,
        confirm: String,
    },
    Start {
        project: String,
        #[serde(default)]
        service: Option<String>,
    },
    Stop {
        project: String,
        #[serde(default)]
        service: Option<String>,
    },
    Restart {
        project: String,
        #[serde(default)]
        service: Option<String>,
    },
    Logs {
        project: String,
        #[serde(default)]
        service: Option<String>,
        #[serde(default)]
        errors: bool,
        #[serde(default = "default_lines")]
        lines: usize,
    },
    Doctor {
        project: String,
    },
    /// Followed by a tar archive on the same connection.
    Deploy {
        project: String,
        /// "none" | "gzip"
        compression: String,
        #[serde(default)]
        message: Option<String>,
    },
    Rollback {
        project: String,
        #[serde(default)]
        to: Option<String>,
    },
    History {
        project: String,
    },
    EnvSet {
        project: String,
        key: String,
        value: String,
    },
    EnvUnset {
        project: String,
        key: String,
    },
    EnvList {
        project: String,
    },
    LimitsSet {
        project: String,
        #[serde(default)]
        memory_mb: Option<u64>,
        #[serde(default)]
        cpu_percent: Option<u64>,
        #[serde(default)]
        pids: Option<u64>,
    },
    AccessGrant {
        user: String,
        project: String,
        permissions: Vec<String>,
    },
    AccessRevoke {
        user: String,
        project: String,
    },
    AccessList,
    Audit {
        #[serde(default = "default_lines")]
        lines: usize,
    },
    /// Create a restricted agent account `agent-<name>` that may only run
    /// nexus commands over SSH.
    AgentCreate {
        name: String,
        public_key: String,
    },
    /// Destructive: requires `confirm == name`.
    AgentRemove {
        name: String,
        confirm: String,
    },
    AgentList,
    SshStatus,
    /// Disable SSH password logins after verifying that the caller has
    /// logged in with a key at least once.
    SshFinalize,
    /// Re-enable password logins (recovery; administrator only).
    SshAllowPasswords,
}

fn default_lines() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Reply {
    Log {
        stream: String,
        line: String,
    },
    Result {
        ok: bool,
        #[serde(default)]
        data: Value,
        #[serde(default)]
        error: Option<ErrorBody>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorBody {
    /// Stable code: FORBIDDEN, NOT_FOUND, INVALID, CONFLICT, FAILED, ...
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: Value,
}

impl Reply {
    pub fn ok(data: Value) -> Reply {
        Reply::Result {
            ok: true,
            data,
            error: None,
        }
    }
    pub fn err(code: &str, message: impl Into<String>) -> Reply {
        Reply::Result {
            ok: false,
            data: Value::Null,
            error: Some(ErrorBody {
                code: code.into(),
                message: message.into(),
                details: Value::Null,
            }),
        }
    }
    pub fn err_details(code: &str, message: impl Into<String>, details: Value) -> Reply {
        Reply::Result {
            ok: false,
            data: Value::Null,
            error: Some(ErrorBody {
                code: code.into(),
                message: message.into(),
                details,
            }),
        }
    }
}

pub fn write_reply(w: &mut (impl Write + ?Sized), r: &Reply) -> io::Result<()> {
    let mut s = serde_json::to_string(r).map_err(io::Error::other)?;
    s.push('\n');
    w.write_all(s.as_bytes())?;
    w.flush()
}

/// Outcome of a request: the final result, after `on_log` saw every event.
pub fn call(
    req: &Request,
    upload: Option<&mut dyn Read>,
    mut on_log: impl FnMut(&str, &str),
) -> io::Result<Result<Value, ErrorBody>> {
    let path = crate::paths::socket();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "cannot reach nexusd at {} ({e}); is the NEXUS supervisor running?",
                path.display()
            ),
        )
    })?;
    let mut line = serde_json::to_string(req).map_err(io::Error::other)?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    if let Some(src) = upload {
        io::copy(src, &mut stream)?;
    }
    stream.shutdown(std::net::Shutdown::Write)?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Reply>(&line) {
            Ok(Reply::Log { stream, line }) => on_log(&stream, &line),
            Ok(Reply::Result { ok, data, error }) => {
                return Ok(if ok {
                    Ok(data)
                } else {
                    Err(error.unwrap_or(ErrorBody {
                        code: "FAILED".into(),
                        message: "unknown error".into(),
                        details: Value::Null,
                    }))
                });
            }
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad reply from nexusd: {e}"),
                ));
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "nexusd closed the connection without a result",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_wire_format() {
        let r = Request::Logs {
            project: "p".into(),
            service: None,
            errors: true,
            lines: 5,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""cmd":"logs""#));
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
        let minimal: Request = serde_json::from_str(r#"{"cmd":"logs","project":"p"}"#).unwrap();
        assert_eq!(
            minimal,
            Request::Logs {
                project: "p".into(),
                service: None,
                errors: false,
                lines: 100
            }
        );
    }
}
