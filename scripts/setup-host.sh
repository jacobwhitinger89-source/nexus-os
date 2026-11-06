#!/bin/bash
# Prepare a Linux x86-64 development host for building NEXUS OS.
# Installs nothing system-wide: missing tools are built or shimmed inside
# the repository (.tools/, scripts/hostshims/) and Rust via rustup in $HOME.
set -euo pipefail
TOP=$(cd "$(dirname "$0")/.." && pwd)
missing=()
for t in gcc g++ make git curl rsync cpio unzip perl python3 file patch tar gzip bzip2 xz qemu-system-x86_64; do
	command -v "$t" >/dev/null || missing+=("$t")
done
if [ ${#missing[@]} -gt 0 ]; then
	echo "Please install these with your package manager: ${missing[*]}" >&2
	echo "  Debian/Ubuntu: sudo apt install build-essential git curl rsync cpio unzip perl python3 file patch bzip2 xz-utils qemu-system-x86 ovmf" >&2
	echo "  Arch:          sudo pacman -S base-devel git curl rsync cpio unzip perl python file patch bzip2 xz qemu-system-x86 edk2-ovmf" >&2
	exit 1
fi

mkdir -p "$TOP/.tools/bin" "$TOP/.tools/src"
if ! command -v bc >/dev/null && [ ! -x "$TOP/.tools/bin/bc" ]; then
	echo "==> building bc (needed by the kernel build) into .tools/bin"
	BCV=7.0.3
	cd "$TOP/.tools/src"
	curl -fsSL -o bc.tar.xz "https://github.com/gavinhoward/bc/releases/download/$BCV/bc-$BCV.tar.xz"
	tar -xf bc.tar.xz
	cd "bc-$BCV" && ./configure -b -O3 -G -N >/dev/null && make -j"$(nproc)" >/dev/null
	cp bin/bc "$TOP/.tools/bin/bc"
fi
if ! command -v wget >/dev/null; then
	echo "==> wget not found: Buildroot will use scripts/hostshims/wget (curl wrapper)"
fi

if ! command -v cargo >/dev/null && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
	echo "==> installing Rust (rustup, user-local)"
	curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path -c rustfmt,clippy
fi
"${CARGO_HOME:-$HOME/.cargo}/bin/rustup" target add x86_64-unknown-linux-musl >/dev/null
echo "==> host ready"
