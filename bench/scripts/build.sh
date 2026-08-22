#!/usr/bin/env bash
# rust-thanos acceptance benchmark: build release binary.
set -euo pipefail
export PATH="/root/.cargo/bin:$PATH"
cd /bench/rust-thanos
cargo build --release --locked 2>&1 | tail -8
ls -la target/release/rust-thanos
echo BUILD_DONE
