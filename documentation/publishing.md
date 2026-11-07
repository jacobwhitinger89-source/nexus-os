# Publishing to GitHub

One script does it: `scripts/publish-github.sh`. It refuses to run unless
the tree is clean and on `main`, checks that no build output (> 5 MB) or
private keys/tokens are tracked, shows the target repository and the commit
author email that will become public, and asks for confirmation.

```sh
gh auth login                              # once, interactive (browser or token)
scripts/publish-github.sh                  # creates github.com/<you>/nexus-os (public) and pushes main
scripts/publish-github.sh --name my-os     # different repository name
scripts/publish-github.sh --tag v0.1.0-alpha.1   # also tag: CI builds the ISO into a draft release
```
Running it again pushes `main` to the existing `origin`.

What happens on GitHub (`.github/workflows/ci.yml`):
* every push: format, clippy, unit tests, host supervisor test, static build;
  then the full image build and the QEMU boot, install, network and USB
  tests (KVM on the hosted runner), plus benchmarks. The ISO and logs are
  uploaded as the `nexus-images` workflow artifact.
* a `v*` tag: the same, then a **draft pre-release** with the ISO, the
  update bundle and `SHA256SUMS`. Check it, then publish it in the GitHub UI
  (see [releasing.md](releasing.md)).

The first image build on CI takes 1–2 hours (Buildroot downloads and the
cross toolchain); later runs reuse the `dl/` cache.

## For AI agent sessions
A new session asked to publish should: run `make test-rust test-host` (and
the QEMU suites if the image changed), commit, then run
`scripts/publish-github.sh` and relay its confirmation prompt to the user —
publishing is public and cannot be fully undone, so never pass `--yes`
without the user's explicit go-ahead. If `gh auth status` fails, ask the user
to run `gh auth login` (in Claude Code: `! gh auth login`).
