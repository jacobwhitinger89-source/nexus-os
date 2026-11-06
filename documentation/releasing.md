# Release process

1. Update `VERSION` and `CHANGELOG.md`; update `DEVELOPMENT_STATUS.md`.
2. `make build && make test` on a clean checkout — all tests must pass.
3. `make bench` and record the numbers in the release notes.
4. Tag: `git tag -s v<version> -m "NEXUS OS <version>"` and push the tag.
5. CI (`.github/workflows/ci.yml`) builds the image on the tag and attaches
   the ISO, update bundle and `SHA256SUMS` to a **draft** GitHub release.
6. Smoke-test the CI-built ISO (`scripts/qemu-run.sh iso`), then publish the
   draft with release notes: highlights, test results, benchmark table,
   known limitations, upgrade notes (`nexus system update`).

Pre-1.0 releases are tagged `-alpha.N` / `-beta.N` and marked pre-release.
