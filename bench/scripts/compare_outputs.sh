#!/usr/bin/env bash
# rust-thanos acceptance benchmark: compare one threshold output's kept
# region/entities/poi file sets against the ground-truth optimized world.
# usage: compare_outputs.sh <output_dir> <label>
set -uo pipefail
SRC=$1
LABEL=$2
BACKUP=/bench/world_backup
WORLD=/bench/world

# Compare *relative* paths (strip each root) so the comm sets line up.
find "$SRC"    -name "*.mca" | sed "s|^$SRC/||"    | sort > "/tmp/${LABEL}_mca.txt"
find "$BACKUP" -name "*.mca" | sed "s|^$BACKUP/||" | sort > "/tmp/backup_mca.txt"
find "$WORLD"  -name "*.mca" | sed "s|^$WORLD/||"  | sort > "/tmp/world_mca.txt"

echo "=== ${LABEL} vs world_backup (ground truth) ==="
echo "output mca files          : $(wc -l < "/tmp/${LABEL}_mca.txt")"
echo "backup mca files          : $(wc -l < /tmp/backup_mca.txt)"
echo "in output but NOT backup  : $(comm -23 "/tmp/${LABEL}_mca.txt" /tmp/backup_mca.txt | wc -l)"
echo "in backup but NOT output  : $(comm -13 "/tmp/${LABEL}_mca.txt" /tmp/backup_mca.txt | wc -l)"
echo "--- files in output not in backup (first 10) ---"
comm -23 "/tmp/${LABEL}_mca.txt" /tmp/backup_mca.txt | head -10
echo "--- files in backup not in output (first 10) ---"
comm -13 "/tmp/${LABEL}_mca.txt" /tmp/backup_mca.txt | head -10

echo "=== ${LABEL} vs world (subset sanity: all output files must exist in world) ==="
echo "in output but NOT world   : $(comm -23 "/tmp/${LABEL}_mca.txt" /tmp/world_mca.txt | wc -l)"
