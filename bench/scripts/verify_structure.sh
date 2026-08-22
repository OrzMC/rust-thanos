#!/usr/bin/env bash
# rust-thanos acceptance benchmark: verify an output world's structure is a
# usable Minecraft world (required dirs/files present, dir counts sane).
# usage: verify_structure.sh <output_dir> <label>
set -uo pipefail
SRC=$1
LABEL=$2

echo "=== structure check: ${LABEL} (${SRC}) ==="
echo "top-level entries:"
ls -A "$SRC" 2>/dev/null | sort
echo "--- dims ---"
find "$SRC/dimensions" -maxdepth 3 -type d 2>/dev/null | sort
echo "--- level.dat present? ---"
[ -f "$SRC/level.dat" ] && echo "yes ($(stat -c %s "$SRC/level.dat") bytes)" || echo "MISSING"
echo "--- players dir file count ---"
find "$SRC/players" -type f 2>/dev/null | wc -l
echo "--- data dir file count ---"
find "$SRC/data" -type f 2>/dev/null | wc -l
echo "--- generated dir file count ---"
find "$SRC/generated" -type f 2>/dev/null | wc -l
echo "--- region/entities/poi file counts ---"
find "$SRC" -path "*/region/*.mca" | wc -l
find "$SRC" -path "*/entities/*.mca" | wc -l
find "$SRC" -path "*/poi/*.mca" | wc -l
echo "--- any 0-byte mca in output? ---"
find "$SRC" -name "*.mca" -size 0 | wc -l
echo "--- total output size ---"
du -sh "$SRC" 2>/dev/null
