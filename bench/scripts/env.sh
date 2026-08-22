#!/usr/bin/env bash
# rust-thanos acceptance benchmark: capture environment snapshot.
set -uo pipefail
OUT=/bench/reports/env.txt
mkdir -p /bench/reports
{
echo "=== rust-thanos acceptance benchmark: environment ==="
echo "date: $(date -u)"
echo "host: $(hostname)  kernel: $(uname -r)"
echo "distro: $(cat /etc/os-release | grep PRETTY_NAME)"
echo "cpu: $(grep 'model name' /proc/cpuinfo | head -1 | sed 's/.*: //')"
echo "cores: $(nproc) logical"
grep -E "^(cpu MHz|cache size|vendor_id)" /proc/cpuinfo | head -3
echo "mem total: $(free -g | awk '/^Mem:/{print $2}') GiB"
echo "rustc: $(/root/.cargo/bin/rustc --version)"
echo "binary: $(/bench/rust-thanos/target/release/rust-thanos --version)"
echo "input world bytes: $(du -sb /bench/world | cut -f1)"
echo "input world mca files: $(find /bench/world -name '*.mca' | wc -l)"
echo "input backup bytes: $(du -sb /bench/world_backup | cut -f1)"
echo "input backup mca files: $(find /bench/world_backup -name '*.mca' | wc -l)"
} > "$OUT" 2>&1
cat "$OUT"
