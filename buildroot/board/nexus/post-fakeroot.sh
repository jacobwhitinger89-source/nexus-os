#!/bin/sh
# NEXUS OS post-fakeroot: runs on each filesystem's private copy of the
# target after Buildroot created users/groups and device nodes.
#
# Identity files live on the data partition because /etc is read-only
# squashfs on installed systems: the generated files become factory
# defaults and /etc/<file> becomes a symlink into /data/etc.
set -eu
TARGET_DIR=$1
factory="$TARGET_DIR/usr/share/nexus/factory/etc"
mkdir -p "$factory"

grep -q '^nexus-agent:' "$TARGET_DIR/etc/group" || echo 'nexus-agent:x:990:' >> "$TARGET_DIR/etc/group"
grep -q '^nexus-svc:' "$TARGET_DIR/etc/group" || echo 'nexus-svc:x:991:' >> "$TARGET_DIR/etc/group"
# No root password; root login is disabled. Administrators use sudo.
sed -i 's#^root:[^:]*:#root:*:#' "$TARGET_DIR/etc/shadow"

for f in passwd group shadow hostname hosts; do
	if [ -f "$TARGET_DIR/etc/$f" ] && [ ! -L "$TARGET_DIR/etc/$f" ]; then
		mv "$TARGET_DIR/etc/$f" "$factory/$f"
	fi
	ln -sfn "/data/etc/$f" "$TARGET_DIR/etc/$f"
done
chmod 0600 "$factory/shadow"
