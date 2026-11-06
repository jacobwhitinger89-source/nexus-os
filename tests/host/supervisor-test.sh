#!/bin/bash
# Host integration test: runs the real nexusd (unprivileged, "limited"
# isolation) against a throw-away NEXUS_ROOT and drives it with the real
# nexus CLI. Exercises project creation, deployment, health checks, crash
# restart, failed-deploy rollback, structured diagnostics and logs.
#
# Usage: tests/host/supervisor-test.sh [bin-dir]   (default target/debug)
set -u
BIN=${1:-target/debug}
BIN=$(cd "$BIN" && pwd)
command -v python3 >/dev/null || { echo "SKIP: python3 needed for the sample app"; exit 0; }

ROOT=$(mktemp -d /tmp/nexus-host-test.XXXXXX)
export NEXUS_ROOT=$ROOT NEXUS_SOCKET=$ROOT/run/nexus/nexusd.sock NEXUS_CGROUP_ROOT=$ROOT/no-cgroups
mkdir -p "$ROOT/etc" "$ROOT/run/nexus"
PORT=$((20000 + RANDOM % 20000))
PASS=0; FAIL=0
ok()   { echo "PASS: $1"; PASS=$((PASS+1)); }
bad()  { echo "FAIL: $1"; FAIL=$((FAIL+1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }
nx() { "$BIN/nexus" "$@"; }
jget() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1]))" "$1"; }

cleanup() {
	[ -n "${DPID:-}" ] && kill "$DPID" 2>/dev/null && wait "$DPID" 2>/dev/null
	pkill -f "http.server $PORT" 2>/dev/null
	rm -rf "$ROOT"
}
trap cleanup EXIT

"$BIN/nexusd" 2>"$ROOT/nexusd.log" &
DPID=$!
for _ in $(seq 50); do [ -S "$NEXUS_SOCKET" ] && break; sleep 0.1; done
check "daemon answers ping" "nx whoami --json | grep -q '\"admin\": true'"

nx project create web --json >/dev/null
check "project created" "nx projects --json | jget 'd[\"projects\"][0][\"status\"]' | grep -qx not_deployed"

# --- sample application ------------------------------------------------------
SRC=$ROOT/src; mkdir -p "$SRC"
cat > "$SRC/nexus.toml" <<EOF
[project]
name = "web"

[build]
command = "echo building; echo ok > built.txt"

[[service]]
name = "http"
command = "python3 -m http.server $PORT --bind 127.0.0.1"
ports = [{ port = $PORT, public = false }]
health = { http = "/built.txt", interval = 1, start_period = 5 }
EOF
OUT=$(tar -czf - -C "$SRC" . | nx deploy web --json)
check "deploy succeeds" "echo \"\$OUT\" | jget 'd[\"deployment\"][\"status\"]' | grep -qx succeeded"
REL1=$(echo "$OUT" | jget 'd["deployment"]["id"]')
check "project active after deploy" "nx project status web --json | jget 'd[\"status\"]' | grep -qx active"
check "app reachable" "curl -fs http://127.0.0.1:$PORT/built.txt | grep -qx ok"

# --- crash restart --------------------------------------------------------------
PID=$(nx project status web --json | jget 'd["services"][0]["pid"]')
kill -9 "$PID"
sleep 4
STATUS=$(nx project status web --json)
check "killed service restarted" "echo \"\$STATUS\" | jget 'd[\"services\"][0][\"restarts\"]' | grep -qx 1"
check "exit signal recorded" "echo \"\$STATUS\" | jget 'd[\"services\"][0][\"last_exit\"][\"signal\"]' | grep -qx 9"
for _ in $(seq 20); do nx project status web --json | jget 'd["status"]' | grep -qx active && break; sleep 0.5; done
check "project active again" "nx project status web --json | jget 'd[\"status\"]' | grep -qx active"

# --- failing deploy rolls back -----------------------------------------------------
sed -i "s|command = \"python3 -m http.server.*|command = \"./does-not-exist\"|" "$SRC/nexus.toml"
OUT=$(tar -czf - -C "$SRC" . | nx deploy web --json); RC=$?
check "broken deploy reports failure" "[ $RC -ne 0 ]"
check "broken deploy rolled back" "echo \"\$OUT\" | jget 'd[\"error\"][\"details\"][\"rolled_back\"]' | grep -qx True"
check "previous release active again" "nx project status web --json | jget 'd[\"release\"][\"id\"]' | grep -qx $REL1"
for _ in $(seq 20); do nx project status web --json | jget 'd["status"]' | grep -qx active && break; sleep 0.5; done
check "service healthy after rollback" "curl -fs http://127.0.0.1:$PORT/built.txt | grep -qx ok"
check "history records rollback" "nx history web --json | jget '[r[\"status\"] for r in d[\"deployments\"]]' | grep -q rolled_back"

# --- build failure ----------------------------------------------------------------
sed -i 's|^command = "echo building.*|command = "echo compiling; exit 3"|' "$SRC/nexus.toml"
OUT=$(tar -czf - -C "$SRC" . | nx deploy web --json)
check "build failure diagnosed" "echo \"\$OUT\" | jget 'd[\"error\"][\"code\"]' | grep -qx BUILD_FAILED"
check "build failure keeps app running" "curl -fs http://127.0.0.1:$PORT/built.txt | grep -qx ok"

# --- port conflict diagnosis -------------------------------------------------
nx stop web --json >/dev/null
for _ in $(seq 20); do nx project status web --json | jget 'd["status"]' | grep -qx inactive && break; sleep 0.3; done
python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1 &
SQUAT=$!
sleep 1
nx start web --json >/dev/null
sleep 3
DOC=$(nx doctor web --json)
kill $SQUAT 2>/dev/null; wait $SQUAT 2>/dev/null
check "doctor reports PORT_IN_USE" "echo \"\$DOC\" | jget '[x[\"code\"] for x in d[\"diagnostics\"]]' | grep -q PORT_IN_USE"
check "diagnostic has recovery actions" "echo \"\$DOC\" | jget 'len(d[\"diagnostics\"][0][\"recovery_actions\"])' | grep -qv '^0$'"

# --- logs -------------------------------------------------------------------------
check "logs contain supervisor events" "nx logs web --json | jget 'd[\"lines\"]' | grep -q 'started (pid'"
check "error log has stderr lines" "nx logs web --errors --json | jget 'len(d[\"lines\"])' | grep -qv '^0$'"
check "audit trail written" "nx audit --json | jget '[e[\"action\"] for e in d[\"entries\"]]' | grep -q deploy"

# --- invalid input ----------------------------------------------------------------
nx project create "Bad_Name" --json >/dev/null 2>&1
check "invalid project name rejected" "[ \$? -eq 1 ]"
nx project delete web --json >/dev/null 2>&1
check "delete without confirmation refused" "nx projects --json | grep -q '\"web\"'"
nx project delete web --confirm web --json >/dev/null
check "delete with confirmation" "! nx projects --json | grep -q '\"web\"'"

echo
echo "host supervisor test: $PASS passed, $FAIL failed"
if [ $FAIL -gt 0 ]; then
	echo "--- nexusd log"; tail -40 "$ROOT/nexusd.log"
	exit 1
fi
