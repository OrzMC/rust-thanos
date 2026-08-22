#!/usr/bin/env bash
# rust-thanos acceptance benchmark: run one backup pass at a given threshold.
# usage: run_one.sh <threshold_seconds> <tag>
set -uo pipefail
export PATH="/root/.cargo/bin:$PATH"
BIN=/bench/rust-thanos/target/release/rust-thanos
WORLD=/bench/world
OUT=/bench/out
REPORTS=/bench/reports

T=$1
TAG=$2
OUTDIR="$OUT/$TAG"

rm -rf "$OUTDIR"
mkdir -p "$REPORTS"

echo "=== run threshold=${T}s tag=${TAG} ==="
/usr/bin/time -v "$BIN" backup "$WORLD" "$OUTDIR" -t "$T" \
  --progress-mode off --parallelism 0 \
  --report-file "$REPORTS/$TAG.json" --report-format json \
  > "$REPORTS/$TAG.stdout" 2> "$REPORTS/$TAG.time"
echo "exit=$?"
echo "=== done ${TAG} ==="
