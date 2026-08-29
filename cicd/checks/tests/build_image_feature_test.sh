#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../../build-image.sh
source "${SCRIPT_DIR}/../../build-image.sh"

assert_fips_detected() {
    GATEWAY_FEATURES=$1
    if ! gateway_features_include fips; then
        echo "ERROR: fips was not detected in feature list: ${GATEWAY_FEATURES}" >&2
        exit 1
    fi
}

assert_fips_absent() {
    GATEWAY_FEATURES=$1
    if gateway_features_include fips; then
        echo "ERROR: approximate feature name was detected as fips: ${GATEWAY_FEATURES}" >&2
        exit 1
    fi
}

assert_fips_detected "allocator-jemalloc,fips"
assert_fips_detected "allocator-jemalloc fips"
assert_fips_detected $'allocator-jemalloc\tfips'
assert_fips_absent "allocator-jemalloc,fips-extra"
assert_fips_absent "allocator-jemalloc,not-fips"

GATEWAY_FEATURES="allocator-jemalloc fips"
if validate_fips_architectures arm64 >/dev/null 2>&1; then
    echo "ERROR: arm64 FIPS architecture validation unexpectedly passed" >&2
    exit 1
fi
validate_fips_architectures amd64

echo "OK: build-image feature parsing and FIPS architecture validation passed"
