#!/usr/bin/env bash
# rust-thanos acceptance benchmark: one dry-run (analysis only, zero writes).
# usage: run_dry.sh <threshold_seconds> <tag>
set -uo pipefail
export PATH="/root/.cargo/bin:$PATH"
BIN=/bench/rust-thanos/target/release/rust-thanos
WORLD=/bench/world
REPORTS=/bench/reports

T=$1
TAG=$2
mkdir -p "$REPORTS"

echo "=== dry-run threshold=${T}s tag=${TAG} ==="
/usr/bin/time -v "$BIN" backup "$WORLD" -t "$T" --dry-run \
  --progress-mode off --parallelism 0 \
  --report-file "$REPORTS/$TAG.json" --report-format json \
  > "$REPORTS/$TAG.stdout" 2> "$REPORTS/$TAG.time"
echo "exit=$?"
echo "=== done ${TAG} ==="
