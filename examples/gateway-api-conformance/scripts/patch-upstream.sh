#!/usr/bin/env bash
# Patch the upstream conformance library's certificate generation in-place.
#
# Go 1.24+ rejects self-signed certificates without IsCA=true when used as
# trust roots.  Upstream conformance v1.5.1 generates such certs, causing all
# TLS-related tests to fail.  Rather than vendoring the entire upstream module
# (525 files) via `go.mod replace`, we patch the single file in the Go module
# cache before building.
#
# This script is idempotent — safe to run multiple times.
#
# Usage (from any script that compiles the conformance tests):
#   source scripts/patch-upstream.sh   # patches module cache in-place
#   go test ./...                      # build uses patched certificate.go
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PATCH_FILE="$CONFORMANCE_ROOT/patches/certificate.go"
if [[ ! -f "$PATCH_FILE" ]]; then
  echo "ERROR: patch file not found: $PATCH_FILE" >&2
  exit 1
fi

GOMODCACHE="$(go env GOMODCACHE)"
UPSTREAM_FILE="$GOMODCACHE/sigs.k8s.io/gateway-api/conformance@v1.5.1/utils/kubernetes/certificate.go"

if [[ ! -f "$UPSTREAM_FILE" ]]; then
  echo "Downloading upstream conformance module into module cache..."
  (cd "$CONFORMANCE_ROOT" && go mod download sigs.k8s.io/gateway-api/conformance@v1.5.1)
fi

if diff -q "$PATCH_FILE" "$UPSTREAM_FILE" &>/dev/null; then
  echo "patch-upstream: certificate.go already patched, skipping."
  return 0 2>/dev/null || exit 0
fi

UPSTREAM_DIR="$(dirname "$UPSTREAM_FILE")"
chmod u+w "$UPSTREAM_DIR" "$UPSTREAM_FILE"
cp "$PATCH_FILE" "$UPSTREAM_FILE"
chmod u-w "$UPSTREAM_FILE" "$UPSTREAM_DIR"

echo "patch-upstream: patched $UPSTREAM_FILE"
