//! `nexus connect` — client-side onboarding for a NEXUS OS server.
//!
//! Runs on the user's or agent's own Linux/macOS machine (never on the
//! server). It fetches the server's ed25519 host key with `ssh-keyscan`,
//! has the user confirm its SHA256 fingerprint against `nexus ssh status`
//! on the server console (or takes `--fingerprint`), pins it in
//! `~/.ssh/known_hosts`, enrolls a *public* key through
//! `nexus ssh add-key` over a normal password SSH login, and writes a marked
//! `Host` block to `~/.ssh/config`. Private keys are never read or sent.
//!
//! Exit codes follow the `nexus` CLI: 0 success, 1 failed (including host
//! key mismatch), 2 usage error, 4 not found (local public key missing),
//! 5 server unreachable.

use base64ct::{Base64, Base64Unpadded, Encoding};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// Must equal `nexus_core::schema::SCHEMA_VERSION` (checked by a CLI test).
pub const SCHEMA_VERSION: u32 = 1;

pub const HELP: &str = "\
nexus connect — set up SSH access to a NEXUS server from this computer

USAGE: nexus connect <host> [options] [--json]
       (or the standalone binary: nexus-connect <host> ...)

  <host>                 server name or address, e.g. pixel.local or 192.168.1.20
  --user NAME            account on the server (default: admin)
  --port N               SSH port (default: 22)
  --alias NAME           ~/.ssh/config alias (default: host without .local)
  --key FILE             public key to enroll (default: ~/.ssh/id_ed25519.pub)
  --fingerprint SHA256:… expected host key fingerprint (non-interactive);
                         read it on the server console: nexus ssh status
  --no-enroll            only pin the host key and write the alias
  --no-config            do not touch ~/.ssh/config
  --ssh-dir DIR          use DIR instead of ~/.ssh (testing)
  --json                 machine-readable result

Steps: fetch host key (ssh-keyscan) -> confirm fingerprint -> add to
known_hosts -> enroll public key via 'nexus ssh add-key' (password login)
-> write a 'Host <alias>' block to ~/.ssh/config. Private keys never leave
this computer.";

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub enum Fail {
    Usage(String),
    NotFound(String),
    Unreachable(String),
    Failed(&'static str, String),
}

impl Fail {
    pub fn exit_code(&self) -> u8 {
        match self {
            Fail::Usage(_) => 2,
            Fail::NotFound(_) => 4,
            Fail::Unreachable(_) => 5,
            Fail::Failed(..) => 1,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            Fail::Usage(_) => "USAGE",
            Fail::NotFound(_) => "NOT_FOUND",
            Fail::Unreachable(_) => "UNREACHABLE",
            Fail::Failed(c, _) => c,
        }
    }

    fn message(&self) -> &str {
        match self {
            Fail::Usage(m) | Fail::NotFound(m) | Fail::Unreachable(m) | Fail::Failed(_, m) => m,
        }
    }
}

fn io_fail(what: &str, e: std::io::Error) -> Fail {
    Fail::Failed("FAILED", format!("{what}: {e}"))
}

// ---------------------------------------------------------- pure helpers

/// A host public key as printed by `ssh-keyscan` / found in known_hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub key_type: String,
    pub blob: String,
}

/// First `ssh-ed25519` key in `ssh-keyscan` output (comment lines skipped).
pub fn parse_keyscan(output: &str) -> Option<HostKey> {
    output.lines().find_map(|l| {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            return None;
        }
        let mut f = l.split_whitespace();
        let (_host, t, blob) = (f.next()?, f.next()?, f.next()?);
        (t == "ssh-ed25519").then(|| HostKey {
            key_type: t.into(),
            blob: blob.into(),
        })
    })
}

/// OpenSSH-style fingerprint `SHA256:<unpadded base64>` of a key blob.
/// Also checks that the blob's embedded type matches `key_type`.
pub fn fingerprint(key: &HostKey) -> Result<String, String> {
    let raw = Base64::decode_vec(&key.blob).map_err(|_| "host key is not valid base64")?;
    let n = raw
        .get(..4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or("host key blob too short")?;
    if raw.get(4..4 + n) != Some(key.key_type.as_bytes()) {
        return Err("host key blob does not match its type".into());
    }
    Ok(format!(
        "SHA256:{}",
        Base64Unpadded::encode_string(&Sha256::digest(&raw))
    ))
}

/// Canonical form for comparing fingerprints typed by a human or printed
/// by dropbearkey: optional `SHA256:` prefix, surrounding space and base64
/// padding are ignored. Base64 is case-sensitive, so case is kept.
pub fn normalize_fingerprint(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("SHA256:").unwrap_or(s);
    s.trim_end_matches('=').to_string()
}

pub fn fingerprints_match(a: &str, b: &str) -> bool {
    let (a, b) = (normalize_fingerprint(a), normalize_fingerprint(b));
    !a.is_empty() && a == b
}

/// Host pattern used by OpenSSH in known_hosts for this host/port.
pub fn known_hosts_pattern(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct KnownHostsCheck {
    /// The exact key is already present for this host.
    pub present: bool,
    /// Plain-text entries for this host and key type with a different key
    /// (e.g. from a reinstall). Left alone; OpenSSH accepts a host when any
    /// matching entry has its key. Hashed entries are not inspected.
    pub stale: usize,
}

pub fn known_hosts_check(existing: &str, pattern: &str, key: &HostKey) -> KnownHostsCheck {
    let mut r = KnownHostsCheck {
        present: false,
        stale: 0,
    };
    for l in existing.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with('@') {
            continue;
        }
        let mut f = l.split_whitespace();
        let (Some(hosts), Some(t), Some(blob)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if t != key.key_type || !hosts.split(',').any(|h| h == pattern) {
            continue;
        }
        if blob == key.blob {
            r.present = true;
        } else {
            r.stale += 1;
        }
    }
    r
}

pub fn known_hosts_line(pattern: &str, key: &HostKey) -> String {
    format!("{pattern} {} {}", key.key_type, key.blob)
}

fn marker_begin(alias: &str) -> String {
    format!("# >>> nexus connect: {alias} >>>")
}

fn marker_end(alias: &str) -> String {
    format!("# <<< nexus connect: {alias} <<<")
}

/// The `~/.ssh/config` block written for one server (markers included,
/// trailing newline). ControlMaster keeps one authenticated connection open
/// so agents running many short `ssh host nexus …` commands stay fast.
pub fn config_block(
    alias: &str,
    host: &str,
    user: &str,
    port: u16,
    identity: Option<&str>,
) -> String {
    let mut b = format!(
        "{}\n# Managed by nexus connect; re-run it to update, delete the block to remove.\nHost {alias}\n    HostName {host}\n    User {user}\n",
        marker_begin(alias)
    );
    if port != 22 {
        b.push_str(&format!("    Port {port}\n"));
    }
    if let Some(id) = identity {
        b.push_str(&format!("    IdentityFile {id}\n"));
    }
    b.push_str(
        "    StrictHostKeyChecking yes\n    ControlMaster auto\n    ControlPath ~/.ssh/nexus-%C\n    ControlPersist 10m\n    ServerAliveInterval 30\n",
    );
    b.push_str(&marker_end(alias));
    b.push('\n');
    b
}

/// Insert or replace this tool's marked block for `alias`. Everything
/// outside the markers is preserved byte for byte. A new block is appended
/// (prepending could capture global options written before the first
/// `Host` line).
pub fn config_update(existing: &str, alias: &str, block: &str) -> Result<String, String> {
    let (b, e) = (marker_begin(alias), marker_end(alias));
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let start = lines.iter().position(|l| l.trim_end() == b);
    let end = lines.iter().position(|l| l.trim_end() == e);
    match (start, end) {
        (Some(s), Some(en)) if s < en => {
            let mut out: String = lines[..s].concat();
            out.push_str(block);
            out.push_str(&lines[en + 1..].concat());
            Ok(out)
        }
        (None, None) => {
            let mut out = existing.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.is_empty() && !out.ends_with("\n\n") {
                out.push('\n');
            }
            out.push_str(block);
            Ok(out)
        }
        _ => Err(format!(
            "the nexus connect block for {alias:?} in the SSH config is damaged (markers missing or out of order); fix or delete it by hand"
        )),
    }
}

/// True when a `Host` line *outside* our managed block already names
/// `alias` (that entry may take precedence; we never edit it).
pub fn config_alias_conflict(existing: &str, alias: &str) -> bool {
    let (b, e) = (marker_begin(alias), marker_end(alias));
    let mut inside = false;
    for l in existing.lines() {
        let t = l.trim();
        if t == b {
            inside = true;
            continue;
        }
        if t == e {
            inside = false;
            continue;
        }
        if inside {
            continue;
        }
        let (kw, rest) = match t.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some(x) => x,
            None => continue,
        };
        if kw.eq_ignore_ascii_case("host")
            && rest
                .trim_start_matches(|c: char| c.is_whitespace() || c == '=')
                .split_whitespace()
                .any(|p| p == alias)
        {
            return true;
        }
    }
    false
}

/// Default alias: the host name without a trailing `.local`.
pub fn default_alias(host: &str) -> String {
    host.strip_suffix(".local").unwrap_or(host).to_string()
}

/// Check that `text` is exactly one OpenSSH public key line (same rules as
/// `nexus ssh add-key`) and return it trimmed.
pub fn validate_public_key(text: &str) -> Result<String, String> {
    if text.contains("PRIVATE KEY") {
        return Err("that is a private key — only the .pub file is ever sent".into());
    }
    let key = text.trim();
    let t = key.split_whitespace().next().unwrap_or("");
    if key.is_empty()
        || key.contains('\n')
        || !(t.starts_with("ssh-") || t.starts_with("ecdsa-") || t.starts_with("sk-"))
        || key.split_whitespace().count() < 2
    {
        return Err("expected one OpenSSH public key line".into());
    }
    Ok(key.to_string())
}

/// Quote `s` as a single POSIX shell word (for the remote command line).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Host names/addresses: no whitespace, no leading '-' (would be read as an
/// ssh option), only characters valid in names and IPv4/IPv6 literals.
pub fn valid_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && !h.starts_with('-')
        && h.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_' | '%'))
}

pub fn valid_user(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 32
        && !u.starts_with('-')
        && u.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn valid_alias(a: &str) -> bool {
    !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `~/.ssh/...` form for the config file when `path` is under `ssh_dir`
/// of the real home; otherwise the absolute path.
fn config_path_display(path: &Path) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

fn rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem / 60) % 60,
        rem % 60
    )
}

/// Same shape as `nexus_core::schema::envelope`.
pub fn envelope(kind: &str, data: Value) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("schema_version".into(), SCHEMA_VERSION.into());
    obj.insert("kind".into(), kind.into());
    obj.insert("generated_at".into(), rfc3339_now().into());
    if let Value::Object(m) = data {
        obj.extend(m);
    }
    Value::Object(obj)
}

// ------------------------------------------------------------- arguments

#[derive(Debug, Default)]
pub struct Options {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub alias: String,
    pub key: Option<PathBuf>,
    pub fingerprint: Option<String>,
    pub no_enroll: bool,
    pub no_config: bool,
    pub ssh_dir: Option<PathBuf>,
    pub json: bool,
    pub help: bool,
}

pub fn parse_args(raw: &[String]) -> Result<Options, String> {
    let mut o = Options {
        user: "admin".into(),
        port: 22,
        ..Default::default()
    };
    let mut alias = None;
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        let (name, inline) = match a.strip_prefix("--") {
            Some(f) => match f.split_once('=') {
                Some((k, v)) => (k, Some(v.to_string())),
                None => (f, None),
            },
            None if a == "-h" => ("help", None),
            None => {
                if !o.host.is_empty() {
                    return Err(format!("unexpected argument {a:?}"));
                }
                o.host = a.clone();
                continue;
            }
        };
        let mut val = || {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| format!("--{name} needs a value"))
        };
        match name {
            "user" => o.user = val()?,
            "port" => {
                o.port = val()?
                    .parse()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or("--port expects a port number")?
            }
            "alias" => alias = Some(val()?),
            "key" => o.key = Some(PathBuf::from(val()?)),
            "fingerprint" => o.fingerprint = Some(val()?),
            "ssh-dir" => o.ssh_dir = Some(PathBuf::from(val()?)),
            "no-enroll" => o.no_enroll = true,
            "no-config" => o.no_config = true,
            "json" => o.json = true,
            "help" => o.help = true,
            other => return Err(format!("unknown option --{other}")),
        }
    }
    if o.help {
        return Ok(o);
    }
    if o.host.is_empty() {
        return Err("missing host (e.g. nexus connect pixel.local)".into());
    }
    if !valid_host(&o.host) {
        return Err(format!("invalid host {:?}", o.host));
    }
    if !valid_user(&o.user) {
        return Err(format!("invalid user name {:?}", o.user));
    }
    o.alias = alias.unwrap_or_else(|| default_alias(&o.host));
    if !valid_alias(&o.alias) {
        return Err(format!(
            "invalid alias {:?} (letters, digits, '.', '-', '_')",
            o.alias
        ));
    }
    if let Some(fp) = &o.fingerprint
        && normalize_fingerprint(fp).is_empty()
    {
        return Err("--fingerprint is empty".into());
    }
    Ok(o)
}

// ------------------------------------------------------------- execution

/// Program names can be overridden for tests (`NEXUS_CONNECT_SSH`,
/// `NEXUS_CONNECT_KEYSCAN`).
fn program(env: &str, default: &str) -> String {
    std::env::var(env).unwrap_or_else(|_| default.to_string())
}

fn say(json: bool, msg: &str) {
    // Progress goes to stderr so --json stdout stays one document.
    if !json || std::io::stderr().is_terminal() {
        eprintln!("{msg}");
    }
}

fn fetch_host_key(o: &Options) -> Result<HostKey, Fail> {
    let prog = program("NEXUS_CONNECT_KEYSCAN", "ssh-keyscan");
    let out = Command::new(&prog)
        .args(["-T", "10", "-t", "ed25519", "-p", &o.port.to_string(), "--"])
        .arg(&o.host)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| {
            Fail::Failed(
                "MISSING_TOOL",
                format!("cannot run {prog} (install OpenSSH client): {e}"),
            )
        })?;
    parse_keyscan(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        Fail::Unreachable(format!(
            "no ed25519 host key from {}:{} (is the server up and reachable? is SSH enabled?)",
            o.host, o.port
        ))
    })
}

fn confirm_fingerprint(o: &Options, fp: &str) -> Result<&'static str, Fail> {
    if let Some(expected) = &o.fingerprint {
        return if fingerprints_match(expected, fp) {
            Ok("flag")
        } else {
            Err(Fail::Failed(
                "HOST_KEY_MISMATCH",
                format!(
                    "host key fingerprint {fp} does not match --fingerprint {expected}; refusing to continue (wrong host or someone in the middle)"
                ),
            ))
        };
    }
    eprint!(
        "\nServer {} presents this ed25519 host key:\n\n    {fp}\n\nOn the server console run 'nexus ssh status' and compare.\nType the last 6 characters, or 'yes' if it matches exactly: ",
        o.host
    );
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| io_fail("reading answer", e))?;
    let ans = line.trim();
    let tail: String = {
        let n = normalize_fingerprint(fp);
        n[n.len().saturating_sub(6)..].to_string()
    };
    if ans == "yes" || (ans.len() == 6 && ans == tail) {
        Ok("interactive")
    } else {
        Err(Fail::Failed(
            "NOT_CONFIRMED",
            "host key not confirmed; nothing was changed".into(),
        ))
    }
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

fn ssh_base(o: &Options, known_hosts: &Path) -> Command {
    let mut c = Command::new(program("NEXUS_CONNECT_SSH", "ssh"));
    c.arg("-p")
        .arg(o.port.to_string())
        .args(["-o", "StrictHostKeyChecking=yes", "-o"])
        .arg(format!("UserKnownHostsFile={}", known_hosts.display()))
        .args(["-o", "ControlMaster=no", "-o", "ControlPath=none"]);
    c
}

pub fn run(o: &Options) -> Result<Value, Fail> {
    let ssh_dir = match &o.ssh_dir {
        Some(d) => d.clone(),
        None => PathBuf::from(
            std::env::var("HOME").map_err(|_| Fail::Failed("FAILED", "HOME not set".into()))?,
        )
        .join(".ssh"),
    };
    let target = format!("{}@{}", o.user, o.host);

    // 0. Local public key first, so we fail before touching anything.
    let pub_path = o
        .key
        .clone()
        .unwrap_or_else(|| ssh_dir.join("id_ed25519.pub"));
    let public_key = if o.no_enroll {
        None
    } else {
        let text = std::fs::read_to_string(&pub_path).map_err(|_| {
            Fail::NotFound(format!(
                "public key {} not found; create one with: ssh-keygen -t ed25519 (or pass --key FILE.pub)",
                pub_path.display()
            ))
        })?;
        Some(validate_public_key(&text).map_err(Fail::Usage)?)
    };
    let priv_path = pub_path
        .to_str()
        .and_then(|s| s.strip_suffix(".pub"))
        .map(PathBuf::from)
        .filter(|p| p.exists());

    // 1–2. Host key + fingerprint confirmation (fail fast without a way
    // to confirm, before any network access).
    if o.fingerprint.is_none() && !std::io::stdin().is_terminal() {
        return Err(Fail::Usage(
            "cannot ask for confirmation without a terminal; pass --fingerprint SHA256:… (from 'nexus ssh status' on the server console)".into(),
        ));
    }
    say(o.json, &format!("fetching host key from {}…", o.host));
    let key = fetch_host_key(o)?;
    let fp = fingerprint(&key).map_err(|e| Fail::Failed("BAD_HOST_KEY", e))?;
    let confirmed_by = confirm_fingerprint(o, &fp)?;

    // 3. known_hosts.
    std::fs::create_dir_all(&ssh_dir).map_err(|e| io_fail("creating ssh dir", e))?;
    set_mode(&ssh_dir, 0o700);
    let kh_path = ssh_dir.join("known_hosts");
    let pattern = known_hosts_pattern(&o.host, o.port);
    let existing = std::fs::read_to_string(&kh_path).unwrap_or_default();
    let check = known_hosts_check(&existing, &pattern, &key);
    if !check.present {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&kh_path)
            .map_err(|e| io_fail("opening known_hosts", e))?;
        let sep = if existing.is_empty() || existing.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        writeln!(f, "{sep}{}", known_hosts_line(&pattern, &key))
            .map_err(|e| io_fail("writing known_hosts", e))?;
        set_mode(&kh_path, 0o600);
    }
    if check.stale > 0 {
        say(
            o.json,
            &format!(
                "note: {} older {} entr{} for {pattern} remain in {}; remove with: ssh-keygen -R '{pattern}'",
                check.stale,
                key.key_type,
                if check.stale == 1 { "y" } else { "ies" },
                kh_path.display()
            ),
        );
    }

    // 4. Enroll the public key (password login; only the .pub line is sent).
    let mut key_login = Value::Null;
    if let Some(pk) = &public_key {
        say(
            o.json,
            &format!(
                "enrolling {} for {target} (enter the account password if asked)…",
                pub_path.display()
            ),
        );
        let out = ssh_base(o, &kh_path)
            .arg("--")
            .arg(&target)
            .arg(format!("nexus ssh add-key {}", shell_quote(pk)))
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|e| {
                Fail::Failed(
                    "MISSING_TOOL",
                    format!("cannot run ssh (install OpenSSH client): {e}"),
                )
            })?;
        if !out.status.success() {
            return Err(Fail::Failed(
                "ENROLL_FAILED",
                format!(
                    "key enrollment over SSH failed (ssh exit {}); check the user name and password, and that password logins are still enabled (nexus ssh status)",
                    out.status.code().unwrap_or(-1)
                ),
            ));
        }
        // Prove the key works without a password.
        if let Some(pp) = &priv_path {
            let ok = ssh_base(o, &kh_path)
                .args([
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "PasswordAuthentication=no",
                    "-o",
                    "KbdInteractiveAuthentication=no",
                    "-o",
                    "IdentitiesOnly=yes",
                    "-i",
                ])
                .arg(pp)
                .arg("--")
                .arg(&target)
                .arg("nexus whoami --json")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            key_login = ok.into();
        }
    }

    // 5. ~/.ssh/config alias block.
    let cfg_path = ssh_dir.join("config");
    let mut conflict = false;
    if !o.no_config {
        let existing = std::fs::read_to_string(&cfg_path).unwrap_or_default();
        conflict = config_alias_conflict(&existing, &o.alias);
        // Only the private key's *path* goes into the config.
        let identity = priv_path.as_deref().map(config_path_display);
        let block = config_block(&o.alias, &o.host, &o.user, o.port, identity.as_deref());
        let updated = config_update(&existing, &o.alias, &block)
            .map_err(|e| Fail::Failed("CONFIG_DAMAGED", e))?;
        if updated != existing {
            std::fs::write(&cfg_path, updated).map_err(|e| io_fail("writing ssh config", e))?;
        }
        set_mode(&cfg_path, 0o600);
        if conflict {
            say(
                o.json,
                &format!(
                    "warning: {} already has its own 'Host {}' entry outside the nexus block; it may take precedence",
                    cfg_path.display(),
                    o.alias
                ),
            );
        }
    }

    let next = if o.no_config {
        format!("ssh -p {} {target} nexus status", o.port)
    } else {
        format!("ssh {} nexus status", o.alias)
    };
    Ok(json!({
        "host": o.host,
        "port": o.port,
        "user": o.user,
        "alias": if o.no_config { Value::Null } else { o.alias.clone().into() },
        "host_key": { "type": key.key_type, "fingerprint": fp, "confirmed_by": confirmed_by },
        "known_hosts": {
            "file": kh_path, "pattern": pattern,
            "added": !check.present, "stale_entries": check.stale
        },
        "key": {
            "file": if o.no_enroll { Value::Null } else { pub_path.display().to_string().into() },
            "enrolled": public_key.is_some(),
            "key_login_verified": key_login
        },
        "ssh_config": {
            "file": if o.no_config { Value::Null } else { cfg_path.display().to_string().into() },
            "written": !o.no_config,
            "alias_conflict": conflict
        },
        "next": next
    }))
}

fn render(v: &Value) -> String {
    let mut s = format!(
        "host key  {}  {} ({})\n",
        v["host_key"]["type"].as_str().unwrap_or(""),
        v["host_key"]["fingerprint"].as_str().unwrap_or(""),
        if v["known_hosts"]["added"] == true {
            "added to known_hosts"
        } else {
            "already in known_hosts"
        }
    );
    if v["key"]["enrolled"] == true {
        s.push_str(&format!(
            "public key {} enrolled{}\n",
            v["key"]["file"].as_str().unwrap_or(""),
            match v["key"]["key_login_verified"].as_bool() {
                Some(true) => "; key login verified",
                Some(false) => "; key login NOT verified (check ssh-agent / key passphrase)",
                None => "",
            }
        ));
    }
    if let Some(a) = v["alias"].as_str() {
        s.push_str(&format!(
            "ssh alias  {a} written to {}\n",
            v["ssh_config"]["file"].as_str().unwrap_or("")
        ));
    }
    s.push_str(&format!(
        "\nnext: {}\nthen, once key login works: ssh {} nexus ssh finalize   (disables password logins)\n",
        v["next"].as_str().unwrap_or(""),
        v["alias"].as_str().unwrap_or("<host>")
    ));
    s
}

/// Entry point shared by `nexus connect …` and the `nexus-connect` binary.
/// `raw` excludes the program name / `connect` word. Returns the exit code.
pub fn main_with(raw: &[String]) -> u8 {
    let json = raw.iter().any(|a| a == "--json");
    let result = parse_args(raw).map_err(Fail::Usage).and_then(|o| {
        if o.help {
            println!("{HELP}");
            Ok(None)
        } else {
            run(&o).map(Some)
        }
    });
    match result {
        Ok(None) => 0,
        Ok(Some(v)) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&envelope("connect", v)).unwrap_or_default()
                );
            } else {
                print!("{}", render(&v));
            }
            0
        }
        Err(f) => {
            if json {
                let e = envelope(
                    "error",
                    json!({ "error": { "code": f.code(), "message": f.message(), "details": null } }),
                );
                println!("{}", serde_json::to_string_pretty(&e).unwrap_or_default());
            } else if matches!(f, Fail::Usage(_)) {
                eprintln!(
                    "nexus connect: {}\nRun 'nexus connect --help' for usage.",
                    f.message()
                );
            } else {
                eprintln!("nexus connect: {}", f.message());
            }
            f.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real ed25519 public key blob (test vector; fingerprint from ssh-keygen -lf).
    const BLOB: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIDEcTN4bg1lzEhqIRmFzeZ5OhHB1hqg1/5lbrybWb55+";

    fn key() -> HostKey {
        HostKey {
            key_type: "ssh-ed25519".into(),
            blob: BLOB.into(),
        }
    }

    #[test]
    fn keyscan_parsing() {
        let out = format!(
            "# pixel.local:22 SSH-2.0-dropbear_2025.88\n[pixel.local]:2222 ecdsa-sha2-nistp256 AAAAE2V\npixel.local ssh-ed25519 {BLOB}\n"
        );
        assert_eq!(parse_keyscan(&out), Some(key()));
        assert_eq!(parse_keyscan("# only comments\n\n"), None);
        assert_eq!(parse_keyscan("h ssh-rsa AAAAB3"), None);
    }

    #[test]
    fn fingerprint_of_known_key() {
        let fp = fingerprint(&key()).unwrap();
        // Same value as `ssh-keygen -lf` prints for this key.
        assert_eq!(fp, "SHA256:MYpnLW8MrEoCUbqpj/MNzwY/D0igh0J4YLv8g1mFlSg");
        // Type mismatch and garbage are rejected.
        let mut k = key();
        k.key_type = "ssh-rsa".into();
        assert!(fingerprint(&k).is_err());
        k.blob = "!!!".into();
        assert!(fingerprint(&k).is_err());
    }

    #[test]
    fn fingerprint_comparison() {
        let fp = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";
        assert!(fingerprints_match(fp, fp));
        assert!(fingerprints_match(
            " 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU= ",
            fp
        ));
        assert!(!fingerprints_match(
            "SHA256:47deqpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU",
            fp
        ));
        assert!(!fingerprints_match("SHA256:", "SHA256:"));
    }

    #[test]
    fn known_hosts_logic() {
        assert_eq!(known_hosts_pattern("pixel.local", 22), "pixel.local");
        assert_eq!(known_hosts_pattern("10.0.0.5", 2222), "[10.0.0.5]:2222");
        let k = key();
        let line = known_hosts_line("pixel.local", &k);
        let text = format!(
            "# comment\nother ssh-ed25519 {BLOB}\n|1|salt|hash ssh-ed25519 AAAA\npixel.local,10.0.0.5 ssh-ed25519 OLDKEY\n"
        );
        assert_eq!(
            known_hosts_check(&text, "pixel.local", &k),
            KnownHostsCheck {
                present: false,
                stale: 1
            }
        );
        let text2 = format!("{text}{line}\n");
        assert!(known_hosts_check(&text2, "pixel.local", &k).present);
        // Different port is a different host for OpenSSH.
        assert!(!known_hosts_check(&text2, "[pixel.local]:2222", &k).present);
        assert!(!known_hosts_check("", "pixel.local", &k).present);
    }

    #[test]
    fn config_block_insert_and_replace() {
        let user = "Host *\n    ServerAliveInterval 60\n\nHost work\n    HostName work.example\n";
        let b1 = config_block(
            "pixel",
            "pixel.local",
            "admin",
            22,
            Some("~/.ssh/id_ed25519"),
        );
        assert!(b1.contains("ControlMaster auto") && b1.contains("ControlPersist"));
        assert!(!b1.contains("Port "));
        let once = config_update(user, "pixel", &b1).unwrap();
        assert!(once.starts_with(user));
        assert!(once.ends_with(&b1));
        // Idempotent.
        assert_eq!(config_update(&once, "pixel", &b1).unwrap(), once);
        // Replace in place, user content around it untouched.
        let tail = "\nHost later\n    User me\n";
        let with_tail = format!("{once}{tail}");
        let b2 = config_block("pixel", "10.0.0.5", "jacob", 2222, None);
        let replaced = config_update(&with_tail, "pixel", &b2).unwrap();
        assert_eq!(replaced, format!("{user}\n{b2}{tail}"));
        assert!(replaced.contains("Port 2222") && !replaced.contains("pixel.local"));
        // Another alias gets its own block.
        let b3 = config_block("lab", "lab.local", "admin", 22, None);
        let both = config_update(&replaced, "lab", &b3).unwrap();
        assert!(both.contains(&b2) && both.ends_with(&b3));
        // Empty file / no trailing newline.
        assert_eq!(config_update("", "pixel", &b1).unwrap(), b1);
        assert_eq!(
            config_update("Host x", "pixel", &b1).unwrap(),
            format!("Host x\n\n{b1}")
        );
        // Damaged markers are refused rather than guessed.
        let damaged = format!("{}\nHost pixel\n", marker_begin("pixel"));
        assert!(config_update(&damaged, "pixel", &b1).is_err());
    }

    #[test]
    fn alias_conflicts() {
        let b = config_block("pixel", "pixel.local", "admin", 22, None);
        assert!(!config_alias_conflict(&b, "pixel"));
        assert!(config_alias_conflict("Host  web pixel\n", "pixel"));
        assert!(config_alias_conflict("host=pixel\n", "pixel"));
        assert!(!config_alias_conflict(
            "Host pixel2\nHostName pixel\n",
            "pixel"
        ));
    }

    #[test]
    fn public_key_validation_and_quoting() {
        assert!(validate_public_key("ssh-ed25519 AAAA me@laptop\n").is_ok());
        assert!(validate_public_key("-----BEGIN OPENSSH PRIVATE KEY-----\nabc").is_err());
        assert!(validate_public_key("ssh-ed25519 A\nssh-ed25519 B").is_err());
        assert!(validate_public_key("hello world").is_err());
        assert!(validate_public_key("ssh-ed25519").is_err());
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn argument_parsing() {
        let p = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let o = p(&["pixel.local"]).unwrap();
        assert_eq!(
            (o.user.as_str(), o.port, o.alias.as_str()),
            ("admin", 22, "pixel")
        );
        let o = p(&[
            "10.0.0.5",
            "--port=2222",
            "--user",
            "jacob",
            "--alias",
            "lab",
            "--fingerprint",
            "SHA256:abc",
            "--json",
        ])
        .unwrap();
        assert_eq!((o.port, o.alias.as_str(), o.json), (2222, "lab", true));
        assert_eq!(o.alias, "lab");
        assert!(p(&[]).is_err());
        assert!(p(&["-oProxyCommand=x"]).is_err());
        assert!(p(&["--", "x"]).is_err());
        assert!(p(&["h", "--port", "0"]).is_err());
        assert!(p(&["h", "--alias", "a*"]).is_err());
        assert!(p(&["h", "--user", "-x"]).is_err());
        assert!(p(&["h", "--bogus"]).is_err());
        assert!(p(&["h", "--key"]).is_err());
        assert!(p(&["--help"]).unwrap().help);
    }

    #[test]
    fn envelope_shape() {
        let e = envelope("connect", json!({ "host": "h" }));
        assert_eq!(e["schema_version"], SCHEMA_VERSION);
        assert_eq!(e["kind"], "connect");
        assert_eq!(e["host"], "h");
        assert!(e["generated_at"].as_str().unwrap().ends_with('Z'));
    }
}
