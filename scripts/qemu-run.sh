#!/bin/bash
# Run NEXUS OS in QEMU. Never touches host disks: virtual disks live in
# build/qemu/.
#
#   scripts/qemu-run.sh live      kernel + initramfs, serial console (fast)
#   scripts/qemu-run.sh iso       boot the installer ISO (UEFI) with a blank 8 GiB disk
#   scripts/qemu-run.sh disk      boot the installed virtual disk (UEFI)
#   scripts/qemu-run.sh disk-bios boot the installed virtual disk (legacy BIOS)
#
# Host port 2222 -> guest SSH, 8080 -> guest 8080. Exit QEMU: Ctrl-A X.
set -euo pipefail
TOP=$(cd "$(dirname "$0")/.." && pwd)
IMG=${IMAGES:-$TOP/build/output/images}
WORK=$TOP/build/qemu
mkdir -p "$WORK"
MODE=${1:-live}
MEM=${MEM:-512}
VERSION=$(cat "$TOP/VERSION")
ISO=$IMG/nexus-$VERSION-x86_64.iso
DISK=$WORK/disk.qcow2

ovmf() {
	for d in /usr/share/edk2/x64 /usr/share/OVMF /usr/share/ovmf/x64 /usr/share/edk2-ovmf/x64 /usr/share/qemu; do
		for c in OVMF_CODE.4m.fd OVMF_CODE.fd; do
			if [ -f "$d/$c" ]; then
				local vars=${c/CODE/VARS}
				[ -f "$WORK/OVMF_VARS.fd" ] || cp "$d/$vars" "$WORK/OVMF_VARS.fd"
				echo "-drive if=pflash,format=raw,readonly=on,file=$d/$c -drive if=pflash,format=raw,file=$WORK/OVMF_VARS.fd"
				return
			fi
		done
	done
	echo "OVMF firmware not found (install edk2-ovmf / ovmf)" >&2
	exit 1
}

ACCEL=(-machine q35)
[ -w /dev/kvm ] && ACCEL+=(-enable-kvm -cpu host) || ACCEL+=(-cpu max)
COMMON=("${ACCEL[@]}" -m "$MEM" -smp 2 -nographic
	-netdev "user,id=n0,hostfwd=tcp::${SSH_PORT:-2222}-:22,hostfwd=tcp::${APP_PORT:-8080}-:8080"
	-device virtio-net-pci,netdev=n0
	-device i6300esb -object rng-random,id=rng0,filename=/dev/urandom -device virtio-rng-pci,rng=rng0)

case "$MODE" in
live)
	exec qemu-system-x86_64 "${COMMON[@]}" -kernel "$IMG/bzImage" -initrd "$IMG/rootfs.cpio.zst" \
		-append "nexus.mode=live console=ttyS0,115200 ${EXTRA_APPEND:-}" ;;
iso)
	[ -f "$DISK" ] || qemu-img create -q -f qcow2 "$DISK" 8G
	rm -f "$WORK/OVMF_VARS.fd"
	# shellcheck disable=SC2046
	exec qemu-system-x86_64 "${COMMON[@]}" $(ovmf) \
		-drive "file=$DISK,if=none,id=d0,format=qcow2" -device virtio-blk-pci,drive=d0,bootindex=0 \
		-drive "file=$ISO,if=none,id=cd,media=cdrom,readonly=on" -device ide-cd,drive=cd,bootindex=1 ;;
disk)
	# shellcheck disable=SC2046
	exec qemu-system-x86_64 "${COMMON[@]}" $(ovmf) \
		-drive "file=$DISK,if=none,id=d0,format=qcow2" -device virtio-blk-pci,drive=d0,bootindex=0 ;;
disk-bios)
	exec qemu-system-x86_64 "${COMMON[@]}" \
		-drive "file=$DISK,if=none,id=d0,format=qcow2" -device virtio-blk-pci,drive=d0,bootindex=0 ;;
*)
	echo "usage: $0 live|iso|disk|disk-bios" >&2; exit 2 ;;
esac
