#!/bin/bash
# Measure NEXUS OS resource usage in QEMU and record JSON results:
# image sizes, boot time, idle memory (live), per-daemon RSS.
set -euo pipefail
TOP=$(cd "$(dirname "$0")/.." && pwd)
IMG=${1:-$TOP/build/output/images}
OUT=$TOP/benchmarks/results
mkdir -p "$OUT"
python3 "$TOP/benchmarks/measure.py" "$IMG" "$OUT/$(date -u +%Y%m%dT%H%M%SZ).json"
