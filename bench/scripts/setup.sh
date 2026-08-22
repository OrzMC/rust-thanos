#!/usr/bin/env bash
# rust-thanos acceptance benchmark: WSL toolchain bootstrap.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

apt-get update -qq
apt-get install -y -qq build-essential curl time >/dev/null

export RUSTUP_HOME=/root/.rustup
export CARGO_HOME=/root/.cargo
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null

export PATH="$CARGO_HOME/bin:$PATH"
rustc --version
cargo --version
echo SETUP_DONE
