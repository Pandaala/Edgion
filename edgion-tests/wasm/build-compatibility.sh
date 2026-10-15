#!/usr/bin/env bash
# Optional SDK probe build; does not install tools or grant WASI capabilities.
set -euo pipefail
# Verified versions: Go 1.24.0, TinyGo 0.38.0, wasm-opt 123.
# Override executable paths without changing global tool installations.
# Resolve overrides before changing directory or PATH, including paths with spaces.
resolve_tool() {
  local tool_path
  tool_path="$(command -v "$1")" || return 1
  printf '%s/%s\n' "$(cd "$(dirname "$tool_path")" && pwd -P)" "$(basename "$tool_path")"
}
GO_BIN="$(resolve_tool "${GO_BIN:-go}")"
TINYGO_BIN="$(resolve_tool "${TINYGO_BIN:-tinygo}")"
WASMOPT="$(resolve_tool "${WASMOPT:-wasm-opt}")"
export WASMOPT
# GO_BIN selects the SDK for both compilers, without ambient GOROOT or automatic
# Go toolchain selection overriding it. TinyGo resolves Go from GOROOT/PATH.
export GOTOOLCHAIN=local
selected_go_root="$(env -u GOROOT "$GO_BIN" env GOROOT)"
GOROOT="$(cd "$selected_go_root" && pwd -P)"
export GOROOT
export PATH="$GOROOT/bin:$PATH"
if [[ "$("$TINYGO_BIN" env GOROOT)" != "$GOROOT" ]]; then
  echo "TinyGo did not select the requested Go SDK" >&2
  exit 1
fi
cd "$(dirname "$0")"
"$GO_BIN" version
"$TINYGO_BIN" version
"${WASMOPT:-wasm-opt}" --version
mkdir -p artifacts
(
  cd tinygo-policy
  "$GO_BIN" mod verify
  "$TINYGO_BIN" build -target=wasi -scheduler=none -o ../artifacts/tinygo_policy.wasm .
)
(
  cd go-reactor-probe
  "$GO_BIN" mod verify
  GOOS=wasip1 GOARCH=wasm "$GO_BIN" build -mod=readonly -trimpath -buildmode=c-shared -o ../artifacts/go_reactor_probe.wasm .
)
