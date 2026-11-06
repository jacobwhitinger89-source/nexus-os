//! Shared building blocks for the NEXUS OS management components:
//! filesystem layout, the versioned JSON schema, project manifests,
//! hardware resource metrics and the nexusd IPC protocol.

pub mod ipc;
pub mod manifest;
pub mod paths;
pub mod resources;
pub mod schema;
pub mod term;
pub mod time;
pub mod users;

/// NEXUS OS release version of these components.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Validate a project or service name: lowercase letters, digits and '-',
/// 1–40 chars, starting with a letter, not ending with '-'.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_lowercase()
        && b[b.len() - 1] != b'-'
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("flight-efb"));
        assert!(valid_name("a1"));
        assert!(!valid_name(""));
        assert!(!valid_name("1abc"));
        assert!(!valid_name("abc-"));
        assert!(!valid_name("ABC"));
        assert!(!valid_name("a/b"));
        assert!(!valid_name("../x"));
        assert!(!valid_name(&"a".repeat(41)));
    }
}
