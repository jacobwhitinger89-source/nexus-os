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

# Identity files live on the data partition; /etc is read-only squashfs.
factory="$TARGET_DIR/usr/share/nexus/factory/etc"
mkdir -p "$factory"
for f in passwd group shadow hostname hosts; do
	if [ -f "$TARGET_DIR/etc/$f" ] && [ ! -L "$TARGET_DIR/etc/$f" ]; then
		mv "$TARGET_DIR/etc/$f" "$factory/$f"
	fi
	ln -sfn "/data/etc/$f" "$TARGET_DIR/etc/$f"
done
chmod 0600 "$factory/shadow"
# Groups NEXUS relies on.
grep -q '^nexus-agent:' "$factory/group" || echo 'nexus-agent:x:990:' >> "$factory/group"
grep -q '^nexus-svc:' "$factory/group" || echo 'nexus-svc:x:991:' >> "$factory/group"

rm -rf "$TARGET_DIR/etc/dropbear"
ln -sfn /data/etc/dropbear "$TARGET_DIR/etc/dropbear"
rm -rf "$TARGET_DIR/var/log"
ln -sfn /data/log "$TARGET_DIR/var/log"
rm -rf "$TARGET_DIR/home"
ln -sfn /data/home "$TARGET_DIR/home"
mkdir -p "$TARGET_DIR/data" "$TARGET_DIR/mnt" "$TARGET_DIR/var/lib"
ln -sfn ../tmp/resolv.conf "$TARGET_DIR/etc/resolv.conf"

# No root password; root login is disabled. Administrators use sudo.
sed -i 's#^root:[^:]*:#root:*:#' "$factory/shadow"
