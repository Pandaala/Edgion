#!/usr/bin/env bash
# Build the reference SDK guests and copy them into artifacts/.
# Prerequisite: rustup target add --toolchain 1.96.1 wasm32-unknown-unknown
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p artifacts
for demo in header lifecycle; do
  cargo +1.96.1 build --locked --manifest-path "${demo}-demo/Cargo.toml" --release --target wasm32-unknown-unknown
  cp "${demo}-demo/target/wasm32-unknown-unknown/release/wasm_${demo}_demo.wasm" "artifacts/${demo}_demo.wasm"
done
