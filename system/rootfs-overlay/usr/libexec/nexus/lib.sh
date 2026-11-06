# Shared shell helpers for NEXUS OS boot and service scripts.
# Sourced, not executed.

NX_RELEASE_FILE=/etc/nexus-release

nx_cmdline() {
	# Print the value of key=value from the kernel command line.
	for arg in $(cat /proc/cmdline); do
		case "$arg" in
		"$1"=*) echo "${arg#*=}"; return 0 ;;
		esac
	done
	return 1
}

nx_mode() { cat /run/nexus/mode 2>/dev/null || echo live; }

nx_banner() {
	. $NX_RELEASE_FILE 2>/dev/null
	echo
	echo "================================================="
	echo "                   NEXUS OS"
	echo "        ${NEXUS_VERSION:-unknown} (${NEXUS_BUILD:-dev})"
	echo "================================================="
	echo
}

nx_step() { printf ' * %-44s' "$1"; }
nx_ok()   { echo "[ OK ]"; }
nx_fail() { echo "[FAIL]"; echo "   -> $1"; logger -t nexus-boot -p daemon.err "$1" 2>/dev/null; }

# /etc on the installed system is read-only (squashfs). Mutable identity
# files are symlinks into /data/etc, seeded here from factory defaults.
nx_prepare_data() {
	umask 022
	mkdir -p /data/etc/nexus /data/etc/dropbear /data/log /data/home \
		/data/nexus/projects /data/nexus/audit /data/var/lib
	chmod 0700 /data/etc/dropbear /data/nexus/audit
	for f in passwd group shadow hostname hosts; do
		[ -e /data/etc/$f ] || cp -p /usr/share/nexus/factory/etc/$f /data/etc/$f
	done
	chmod 0600 /data/etc/shadow
	for f in /usr/share/nexus/factory/etc/nexus/*; do
		[ -e "$f" ] || continue
		[ -e /data/etc/nexus/${f##*/} ] || cp -p "$f" /data/etc/nexus/
	done
	chmod 0711 /data/home
}

nx_zram_setup() {
	# Compressed swap in RAM helps small machines survive memory spikes.
	# ZRAM_ENABLE=auto enables it when RAM < 4 GiB; yes/no force it.
	ZRAM_ENABLE=auto
	[ -f /data/etc/nexus/zram.conf ] && . /data/etc/nexus/zram.conf
	[ -e /sys/block/zram0 ] || return 0
	memkb=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)
	case "$ZRAM_ENABLE" in
	no) return 0 ;;
	auto) [ "$memkb" -lt 4194304 ] || return 0 ;;
	esac
	echo zstd > /sys/block/zram0/comp_algorithm 2>/dev/null
	echo $((memkb / 2))K > /sys/block/zram0/disksize || return 0
	mkswap /dev/zram0 >/dev/null 2>&1 && swapon -p 100 /dev/zram0 2>/dev/null
}

nx_sysinfo() {
	cpu=$(awk -F': ' '/^model name/ {print $2; exit}' /proc/cpuinfo)
	cores=$(grep -c '^processor' /proc/cpuinfo)
	memtotal=$(awk '/^MemTotal:/ {printf "%d", $2/1024}' /proc/meminfo)
	memavail=$(awk '/^MemAvailable:/ {printf "%d", $2/1024}' /proc/meminfo)
	echo
	echo " HARDWARE"
	echo " -------------------------------------------------"
	echo " Kernel:     $(uname -r)"
	echo " CPU:        ${cpu:-unknown} (${cores} logical cores)"
	echo " Memory:     ${memtotal} MiB total, ${memavail} MiB available"
	for b in /sys/block/*; do
		n=${b##*/}
		case $n in loop*|ram*|zram*) continue ;; esac
		[ -r $b/size ] || continue
		size=$(( $(cat $b/size) / 2048 ))
		model=$(cat $b/device/model 2>/dev/null | tr -s ' ')
		echo " Disk:       /dev/$n ${size} MiB ${model}"
	done
	for n in /sys/class/net/*; do
		i=${n##*/}
		[ "$i" = lo ] && continue
		echo " Network:    $i ($(cat $n/address 2>/dev/null)) link $(cat $n/operstate 2>/dev/null)"
	done
	echo " Boot mode:  $(nx_mode) $(cat /run/nexus/slot 2>/dev/null)"
	echo
}
