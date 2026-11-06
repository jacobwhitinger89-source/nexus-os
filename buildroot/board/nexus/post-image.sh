#!/bin/sh
# NEXUS OS post-image: assemble the hybrid BIOS/UEFI installer ISO.
#
# ISO layout:
#   /boot/bzImage, /boot/initrd.zst     live installer system
#   /boot/grub/                         GRUB config + BIOS El Torito image
#   /nexus/                             install payload (system slot image,
#                                       kernel, SHA256SUMS)
#   appended FAT partition               EFI/BOOT/BOOTX64.EFI
set -eu
BOARD_DIR=$(cd "$(dirname "$0")" && pwd)
NEXUS_ROOT=$(cd "$BOARD_DIR/../../.." && pwd)
VERSION=$(cat "$NEXUS_ROOT/VERSION")
B=$BINARIES_DIR
GRUB_BUILD=$(ls -d "$BUILD_DIR"/grub2-*/ | head -n1)
PC_DIR=$GRUB_BUILD/build-i386-pc/grub-core
EFI_DIR=$GRUB_BUILD/build-x86_64-efi/grub-core
MKIMAGE=$HOST_DIR/bin/grub-mkimage
STAGE=$B/iso-stage
ISO=$B/nexus-$VERSION-x86_64.iso

rm -rf "$STAGE"
mkdir -p "$STAGE/boot/grub/i386-pc" "$STAGE/nexus"

install -m 0644 "$B/bzImage" "$STAGE/boot/bzImage"
install -m 0644 "$B/rootfs.cpio.zst" "$STAGE/boot/initrd.zst"
install -m 0644 "$BOARD_DIR/grub-live.cfg" "$STAGE/boot/grub/grub.cfg"
echo "NEXUS OS $VERSION installer media" > "$STAGE/boot/nexus-live.id"

# Install payload, verified by the installer before writing to disk.
install -m 0644 "$B/rootfs.squashfs" "$STAGE/nexus/system.squashfs"
install -m 0644 "$B/bzImage" "$STAGE/nexus/bzImage"
echo "$VERSION" > "$STAGE/nexus/VERSION"
(cd "$STAGE/nexus" && sha256sum system.squashfs bzImage VERSION > SHA256SUMS)

EARLY=$B/grub-early.cfg
cat > "$EARLY" <<CFG
search --no-floppy --set=root --file /boot/nexus-live.id
set prefix=(\$root)/boot/grub
configfile \$prefix/grub.cfg
CFG

COMMON_MODS="normal linux configfile search search_fs_file echo test \
	part_msdos part_gpt iso9660 fat serial terminal reboot halt"

# BIOS: El Torito image (cdboot.img + core.img).
"$MKIMAGE" -d "$PC_DIR" -O i386-pc -p /boot/grub -c "$EARLY" \
	-o "$B/core-iso.img" biosdisk $COMMON_MODS
cat "$PC_DIR/cdboot.img" "$B/core-iso.img" > "$STAGE/boot/grub/i386-pc/eltorito.img"

# UEFI: removable-media loader in a small FAT image.
"$MKIMAGE" -d "$EFI_DIR" -O x86_64-efi -p /boot/grub -c "$EARLY" \
	-o "$B/BOOTX64-iso.EFI" $COMMON_MODS efi_gop all_video
EFIIMG=$STAGE/boot/grub/efi.img
rm -f "$EFIIMG"
dd if=/dev/zero of="$EFIIMG" bs=1k count=4096 status=none
"$HOST_DIR/sbin/mkfs.vfat" -n NEXUS_EFI "$EFIIMG" >/dev/null
"$HOST_DIR/bin/mmd" -i "$EFIIMG" ::/EFI ::/EFI/BOOT
"$HOST_DIR/bin/mcopy" -i "$EFIIMG" "$B/BOOTX64-iso.EFI" ::/EFI/BOOT/BOOTX64.EFI

rm -f "$ISO"
"$HOST_DIR/bin/xorriso" -as mkisofs -quiet -r -J -V NEXUS_LIVE \
	-b boot/grub/i386-pc/eltorito.img -no-emul-boot -boot-load-size 4 \
	-boot-info-table --grub2-boot-info \
	--grub2-mbr "$PC_DIR/boot_hybrid.img" \
	--efi-boot boot/grub/efi.img -efi-boot-part --efi-boot-image \
	--protective-msdos-label \
	-o "$ISO" "$STAGE"

# A/B update bundle for "nexus system update": same payload as the ISO.
BUNDLE=$B/nexus-$VERSION-x86_64-update.tar
tar -C "$STAGE/nexus" --owner=0 --group=0 --numeric-owner \
	-cf "$BUNDLE" system.squashfs bzImage VERSION SHA256SUMS

(cd "$B" && sha256sum "$(basename "$ISO")" "$(basename "$BUNDLE")" > SHA256SUMS)
echo "NEXUS: wrote $ISO"
echo "NEXUS: wrote $BUNDLE"
