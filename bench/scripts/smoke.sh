#!/usr/bin/env bash
# rust-thanos acceptance benchmark: smoke test on the optimized world.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN=/bench/rust-thanos/target/release/rust-thanos
SRC=/bench/world_backup
OUT=/bench/out/smoke_backup
REPORTS=/bench/reports
mkdir -p "$REPORTS"
rm -rf "$OUT"

echo "=== smoke: backup world_backup -t 300 ==="
/usr/bin/time -v "$BIN" backup "$SRC" "$OUT" -t 300 \
  --progress-mode off --parallelism 0 \
  --report-file "$REPORTS/smoke_backup.json" --report-format json \
  > "$REPORTS/smoke_backup.stdout" 2> "$REPORTS/smoke_backup.time"
echo "exit=$?"
echo "--- stdout ---"
cat "$REPORTS/smoke_backup.stdout"
echo "--- time -v (selected) ---"
grep -E "wall clock|Maximum resident" "$REPORTS/smoke_backup.time"
echo "--- report ---"
cat "$REPORTS/smoke_backup.json"
echo ""
echo "=== structure check ==="
bash "$DIR/verify_structure.sh" "$OUT" smoke
