#!/bin/sh
# NEXUS OS post-build: runs on every build against $TARGET_DIR (idempotent).
set -eu
TARGET_DIR=$1
NEXUS_ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
VERSION=$(cat "$NEXUS_ROOT/VERSION")
BUILD_ID=$(git -C "$NEXUS_ROOT" rev-parse --short HEAD 2>/dev/null || echo unversioned)

cat > "$TARGET_DIR/etc/nexus-release" <<REL
NEXUS_VERSION=$VERSION
NEXUS_BUILD=$BUILD_ID
REL

cat > "$TARGET_DIR/usr/lib/os-release" <<REL
NAME="NEXUS OS"
ID=nexus
VERSION_ID=$VERSION
PRETTY_NAME="NEXUS OS $VERSION"
HOME_URL="https://github.com/nexus-os/nexus"
BUILD_ID=$BUILD_ID
REL
ln -sf ../usr/lib/os-release "$TARGET_DIR/etc/os-release"

# BusyBox init starts nothing from /etc/init.d/S??*; NEXUS owns startup
# through /etc/inittab and /usr/libexec/nexus/svc.
rm -f "$TARGET_DIR"/etc/init.d/S[0-9][0-9]*

rm -rf "$TARGET_DIR/etc/dropbear"
ln -sfn /data/etc/dropbear "$TARGET_DIR/etc/dropbear"
rm -rf "$TARGET_DIR/var/log"
ln -sfn /data/log "$TARGET_DIR/var/log"
rm -rf "$TARGET_DIR/home"
ln -sfn /data/home "$TARGET_DIR/home"
mkdir -p "$TARGET_DIR/data" "$TARGET_DIR/mnt" "$TARGET_DIR/var/lib"
ln -sfn ../tmp/resolv.conf "$TARGET_DIR/etc/resolv.conf"

# Drop tools a server never runs. grub-install/-editenv/-probe/-bios-setup
# and the module directories stay: the installer and A/B updates use them.
for f in grub-mkrescue grub-fstest grub-render-label grub-file grub-syslinux2cfg \
	grub-mkstandalone grub-mknetdir grub-mkrelpath grub-mklayout grub-script-check \
	grub-mkpasswd-pbkdf2 grub-glue-efi grub-kbdcomp grub-macbless grub-menulst2cfg \
	grub-mount grub-ofpathname grub-sparc64-setup grub-reboot grub-set-default \
	grub-mkconfig grub-mkfont grub-mkimage grub-mknetdir cvtsudoers sudoreplay; do
	rm -f "$TARGET_DIR/usr/bin/$f" "$TARGET_DIR/usr/sbin/$f"
done
rm -rf "$TARGET_DIR/usr/share/locale" "$TARGET_DIR/etc/grub.d" "$TARGET_DIR/usr/share/grub/themes"
rm -f "$TARGET_DIR"/usr/lib/libgomp.so* "$TARGET_DIR/usr/sbin/smartd"
