#!/usr/bin/env bash
# rust-thanos acceptance benchmark: merge the optimized world back into the
# full world, then verify the round-trip (restored == full world at chunk level).
set -uo pipefail
BIN=/bench/rust-thanos/target/release/rust-thanos
BASE=/bench/world
PATCH=/bench/world_backup
OUT=/bench/out/merged
REPORTS=/bench/reports
mkdir -p "$REPORTS"
rm -rf "$OUT"

echo "=== merge: base=world patch=world_backup ==="
/usr/bin/time -v "$BIN" merge "$BASE" "$PATCH" "$OUT" -f \
  --parallelism 12 --progress-mode off \
  --report-file "$REPORTS/merge.json" --report-format json \
  > "$REPORTS/merge.stdout" 2> "$REPORTS/merge.time"
echo "exit=$?"
echo "--- stdout ---"
cat "$REPORTS/merge.stdout"
echo "--- time -v (selected) ---"
grep -E "wall clock|Maximum resident" "$REPORTS/merge.time"
echo "--- report ---"
cat "$REPORTS/merge.json"
echo ""
echo "=== round-trip chunk-count check: dry-run restored vs world (both at -t 300) ==="
"$BIN" backup "$OUT" --dry-run -t 300 --progress-mode off --parallelism 0 \
  --report-file "$REPORTS/merged_dry300.json" --report-format json \
  > "$REPORTS/merged_dry300.stdout" 2>&1
cat "$REPORTS/merged_dry300.json"
echo ""
echo "=== restored vs world: region file set (relative paths) ==="
find "$OUT"  -name "*.mca" | sed "s|^$OUT/||"  | sort > /tmp/merged_mca.txt
find "$BASE" -name "*.mca" | sed "s|^$BASE/||" | sort > /tmp/world_mca2.txt
echo "restored mca: $(wc -l < /tmp/merged_mca.txt)"
echo "world   mca : $(wc -l < /tmp/world_mca2.txt)"
echo "in world not in restored (should be 0): $(comm -23 /tmp/world_mca2.txt /tmp/merged_mca.txt | wc -l)"
echo "in restored not in world (should be 0): $(comm -13 /tmp/world_mca2.txt /tmp/merged_mca.txt | wc -l)"
