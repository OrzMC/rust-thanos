#!/usr/bin/env bash
# rust-thanos acceptance benchmark: threshold sweep over the full world.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
echo "=== SWEEP START ==="
date -u

run_one () {
  local t=$1 tag=$2
  echo "############ run -t $t ($tag) ############"
  bash "$DIR/run_one.sh" "$t" "$tag"
  echo "---- report $tag ----"
  cat "/bench/reports/$tag.json"
  echo ""
  echo "---- time $tag ----"
  grep -E "wall clock|Maximum resident" "/bench/reports/$tag.time"
}

run_one 0    t0
run_one 60   t60
run_one 120  t120
run_one 180  t180
run_one 240  t240
run_one 300  t300

echo "=== SWEEP DONE ==="
date -u
echo "=== summary ==="
python3 "$DIR/summarize.py" /bench/reports/t*.json
