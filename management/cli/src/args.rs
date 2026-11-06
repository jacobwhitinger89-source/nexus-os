//! Tiny argument parser: positional words plus `--flag` / `--key value`.

use std::collections::BTreeMap;

pub struct Args {
    pub pos: Vec<String>,
    pub flags: BTreeMap<String, Option<String>>,
}

/// Flags that take a value.
const VALUED: &[&str] = &[
    "service",
    "lines",
    "source",
    "message",
    "to",
    "memory",
    "cpu",
    "pids",
    "key",
    "key-file",
    "confirm",
    "address",
    "gateway",
    "dns",
    "interface",
];

impl Args {
    pub fn parse(raw: impl IntoIterator<Item = String>) -> Result<Args, String> {
        let mut pos = Vec::new();
        let mut flags = BTreeMap::new();
        let mut it = raw.into_iter().peekable();
        while let Some(a) = it.next() {
            if a == "--" {
                pos.extend(it.by_ref());
                break;
            }
            if let Some(f) = a.strip_prefix("--") {
                if let Some((k, v)) = f.split_once('=') {
                    flags.insert(k.to_string(), Some(v.to_string()));
                } else if VALUED.contains(&f) {
                    let v = it.next().ok_or_else(|| format!("--{f} needs a value"))?;
                    flags.insert(f.to_string(), Some(v));
                } else {
                    flags.insert(f.to_string(), None);
                }
            } else if a == "-h" {
                flags.insert("help".into(), None);
            } else {
                pos.push(a);
            }
        }
        Ok(Args { pos, flags })
    }

    pub fn has(&self, f: &str) -> bool {
        self.flags.contains_key(f)
    }

    pub fn get(&self, f: &str) -> Option<&str> {
        self.flags.get(f).and_then(|v| v.as_deref())
    }

    pub fn num(&self, f: &str) -> Result<Option<u64>, String> {
        self.get(f)
            .map(|v| {
                v.parse::<u64>()
                    .map_err(|_| format!("--{f} expects a number"))
            })
            .transpose()
    }

    pub fn word(&self, i: usize) -> Option<&str> {
        self.pos.get(i).map(String::as_str)
    }

    pub fn need(&self, i: usize, what: &str) -> Result<&str, String> {
        self.word(i).ok_or_else(|| format!("missing {what}"))
    }
}

/// Split a command line the way a POSIX shell would for simple quoting
/// ('single', "double", backslash). No expansion of any kind.
pub fn shell_words(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("unterminated single quote".into()),
                    }
                }
            }
            '"' => {
                have = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x) => cur.push(x),
                            None => return Err("dangling backslash".into()),
                        },
                        Some(x) => cur.push(x),
                        None => return Err("unterminated double quote".into()),
                    }
                }
            }
            '\\' => {
                have = true;
                cur.push(chars.next().ok_or("dangling backslash")?);
            }
            c if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            ';' | '|' | '&' | '`' | '$' | '<' | '>' | '(' | ')' => {
                return Err(format!("shell metacharacter {c:?} is not allowed"));
            }
            c => {
                have = true;
                cur.push(c);
            }
        }
    }
    if have {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flags() {
        let a = Args::parse(
            [
                "logs",
                "web",
                "--errors",
                "--lines",
                "5",
                "--service=api",
                "--json",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(a.pos, vec!["logs", "web"]);
        assert!(a.has("errors") && a.has("json"));
        assert_eq!(a.get("lines"), Some("5"));
        assert_eq!(a.get("service"), Some("api"));
        assert_eq!(a.num("lines").unwrap(), Some(5));
        assert!(Args::parse(["--lines".to_string()]).is_err());
    }

    #[test]
    fn words() {
        assert_eq!(
            shell_words(r#"nexus logs  'my app' --message "a \"b\"""#).unwrap(),
            vec!["nexus", "logs", "my app", "--message", "a \"b\""]
        );
        assert!(shell_words("nexus status; rm -rf /").is_err());
        assert!(shell_words("nexus $(id)").is_err());
        assert!(shell_words("nexus 'x").is_err());
        assert_eq!(shell_words("").unwrap(), Vec::<String>::new());
    }
}
