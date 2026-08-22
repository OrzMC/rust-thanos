#!/usr/bin/env bash
# Emit one line per completed threshold report; exit when the sweep finishes.
set -u
prev=""
while :; do
  cur=$(ls /bench/reports/t[0-9]*.json 2>/dev/null | sort)
  for f in $cur; do
    case " $prev " in *" $f "*) ;; *) echo "REPORT: $f";; esac
  done
  prev=$cur
  cnt=$(echo "$cur" | grep -c . || true)
  if [ "${cnt:-0}" -ge 6 ] && ! pgrep -f "sweep.sh" >/dev/null 2>&1; then
    echo "SWEEP_COMPLETE"
    break
  fi
  sleep 20
done
