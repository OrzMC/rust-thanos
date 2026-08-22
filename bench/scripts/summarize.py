#!/usr/bin/env python3
"""Aggregate rust-thanos backup report JSONs into a comparison table."""
import json, glob, os, sys

def hsize(n):
    for unit in ["B", "KB", "MB", "GB", "TB"]:
        if n < 1024 or unit == "TB":
            return f"{n:.2f} {unit}" if unit != "B" else f"{n} B"
        n /= 1024.0

def main():
    pat = sys.argv[1] if len(sys.argv) > 1 else "/bench/reports/*.json"
    rows = []
    for path in sorted(glob.glob(pat)):
        tag = os.path.splitext(os.path.basename(path))[0]
        if tag.endswith(".time") or tag.endswith(".stdout"):
            continue
        try:
            d = json.load(open(path))
        except Exception as e:
            print(f"SKIP {tag}: {e}")
            continue
        rows.append((tag, d))
    if not rows:
        print("no reports")
        return
    # header
    print(f"{'tag':<12}{'proc':>12}{'removed':>12}{'kept':>12}{'before':>10}{'after':>10}{'red%':>8}{'durMs':>10}{'chunks/s':>10}{'MB/s':>9}{'err':>5}")
    for tag, d in sorted(rows):
        proc = d.get("processedChunks", 0)
        rem = d.get("removedChunks", 0)
        kept = d.get("keptChunks", 0)
        before = d.get("beforeSize", 0)
        after = d.get("afterSize", 0)
        dur = d.get("durationMs", 0)
        errs = len(d.get("errors", []))
        red = (1 - after / before) * 100 if before else 0
        cps = proc / (dur / 1000) if dur else 0
        mbs = (before - after) / (dur / 1000) / 1048576 if dur else 0
        print(f"{tag:<12}{proc:>12}{rem:>12}{kept:>12}{hsize(before):>10}{hsize(after):>10}{red:>7.2f}%{dur:>10}{cps:>10.0f}{mbs:>8.1f}{errs:>5}")

if __name__ == "__main__":
    main()
