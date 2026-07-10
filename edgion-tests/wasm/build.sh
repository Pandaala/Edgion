#!/usr/bin/env bash
# Build the reference guest module and copy it into artifacts/.
# Prerequisite: rustup target add wasm32-unknown-unknown
set -euo pipefail
cd "$(dirname "$0")/header-demo"
cargo build --release --target wasm32-unknown-unknown
mkdir -p ../artifacts
cp target/wasm32-unknown-unknown/release/wasm_header_demo.wasm ../artifacts/header_demo.wasm
echo "artifact: $(cd .. && pwd)/artifacts/header_demo.wasm"
