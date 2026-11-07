#!/bin/sh
# Publish this repository to a public GitHub repository (first time), or
# push main to the existing one; optionally tag a release so CI builds the
# ISO and attaches it to a draft GitHub release.
#
#   scripts/publish-github.sh [--name NAME] [--private] [--tag vX.Y.Z-alpha.N] [--yes]
#
# Requires: gh (logged in: `gh auth login`), a clean working tree on main.
set -eu
cd "$(dirname "$0")/.."

name=nexus-os
visibility=--public
tag=
yes=
while [ $# -gt 0 ]; do
	case "$1" in
	--name) name=$2; shift 2 ;;
	--private) visibility=--private; shift ;;
	--tag) tag=$2; shift 2 ;;
	--yes) yes=1; shift ;;
	-h|--help) sed -n '2,9p' "$0"; exit 0 ;;
	*) echo "unknown option: $1" >&2; exit 2 ;;
	esac
done

die() { echo "error: $*" >&2; exit 1; }

command -v gh >/dev/null || die "gh (GitHub CLI) is not installed"
gh auth status >/dev/null 2>&1 || die "not logged in to GitHub: run 'gh auth login'"
[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || die "not on branch main"
[ -z "$(git status --porcelain)" ] || die "working tree has uncommitted changes"

echo "==> Checking tracked files"
big=$(git ls-files -z | xargs -0 stat -c '%s %n' | awk '$1 > 5000000 {print $2}')
[ -z "$big" ] || die "files over 5 MB are tracked (build output?): $big"
# Private keys or tokens in tracked files (the connect test's literal
# rejected-key string is the only allowed match).
leaks=$(git grep -nIE 'BEGIN [A-Z ]*PRIVATE KEY|gh[pousr]_[A-Za-z0-9]{30,}|sk-ant-[A-Za-z0-9-]{20,}|AKIA[0-9A-Z]{16}' \
	-- ':!management/connect/src/lib.rs' || true)
[ -z "$leaks" ] || die "possible secrets in tracked files:
$leaks"
echo "    ok: $(git ls-files | wc -l) files, no build output, no keys"

owner=$(gh api user --jq .login)
email=$(git log -1 --format=%ae)
echo "==> Target: github.com/$owner/$name ($visibility)"
echo "    Commits will be public with author email: $email"
if [ -z "$yes" ]; then
	printf 'Continue? [y/N] '
	read -r ans
	[ "$ans" = y ] || [ "$ans" = Y ] || die "aborted"
fi

if git remote get-url origin >/dev/null 2>&1; then
	echo "==> Pushing main to $(git remote get-url origin)"
	git push origin main
else
	echo "==> Creating github.com/$owner/$name"
	gh repo create "$name" $visibility --source . --remote origin --push \
		--description "Minimal installable server OS (Buildroot + Linux) managed over SSH by humans and AI agents"
fi

if [ -n "$tag" ]; then
	echo "==> Tagging $tag (CI builds the ISO and drafts a release)"
	git tag -a "$tag" -m "NEXUS OS $tag"
	git push origin "$tag"
fi

url=$(gh repo view --json url --jq .url)
echo
echo "Published: $url"
echo "CI runs:   $url/actions"
[ -z "$tag" ] || echo "Release:   $url/releases (draft; publish it after checking the CI-built ISO)"
