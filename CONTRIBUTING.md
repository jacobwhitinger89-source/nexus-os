# Contributing

Thank you for helping. NEXUS OS aims to stay small, reliable and secure.

* Discuss larger changes in an issue first.
* Keep the base OS minimal: new packages need a clear server use case and
  a note on their RAM/disk cost.
* Rust: `cargo fmt`, `cargo clippy -- -D warnings`, tests alongside code.
  No async runtime or heavy dependencies in nexusd without discussion.
* Never claim a feature works without a test; update
  `DEVELOPMENT_STATUS.md`, `KNOWN_LIMITATIONS.md` and docs with behaviour
  changes.
* JSON output is an API: only add fields within a schema version; document
  them in `documentation/json-schema.md`.
* Security-relevant changes (auth, sandbox, SSH, firewall, installer) need a
  short threat note in the PR. No custom cryptography.
* Never commit credentials, keys or generated images.
* Before a PR: `make test-rust test-host`; for system changes also
  `make build test-boot test-install`.

Commits: imperative subject, explain *why* in the body. By contributing you
agree your work is licensed under Apache-2.0.
