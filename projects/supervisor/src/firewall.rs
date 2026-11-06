//! Keeps the nftables sets `inet nexus public_tcp/public_udp` equal to the
//! public ports of running services (see /etc/nexus/firewall.nft).

use std::collections::BTreeSet;
use std::process::{Command, Stdio};

fn nft(script: &str) -> bool {
    let child = Command::new("nft")
        .args(["-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(script.as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

pub fn script(ports: &BTreeSet<(String, u16)>) -> String {
    let mut s = String::from("flush set inet nexus public_tcp\nflush set inet nexus public_udp\n");
    for proto in ["tcp", "udp"] {
        let list: Vec<String> = ports
            .iter()
            .filter(|(p, _)| p == proto)
            .map(|(_, port)| port.to_string())
            .collect();
        if !list.is_empty() {
            s.push_str(&format!(
                "add element inet nexus public_{proto} {{ {} }}\n",
                list.join(", ")
            ));
        }
    }
    s
}

/// Apply the set atomically (one nft transaction). Returns success.
pub fn apply(ports: &BTreeSet<(String, u16)>) -> bool {
    nft(&script(ports))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_script() {
        let mut set = BTreeSet::new();
        set.insert(("tcp".to_string(), 3000));
        set.insert(("tcp".to_string(), 80));
        let s = script(&set);
        assert!(s.contains("add element inet nexus public_tcp { 80, 3000 }"));
        assert!(!s.contains("public_udp {"));
    }
}
