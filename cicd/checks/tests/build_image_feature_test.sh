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

if [[ "$(builder_image_tag linux/arm64)" != "edgion-builder:linux-arm64" ]]; then
    echo "ERROR: ordinary arm64 builder tag changed unexpectedly" >&2
    exit 1
fi
if [[ "$(builder_image_tag linux/arm64 true)" != "edgion-builder:linux-arm64-fips" ]]; then
    echo "ERROR: arm64 FIPS builder does not use an isolated tag" >&2
    exit 1
fi
if [[ "$(builder_image_tag linux/amd64 true)" != "edgion-builder:linux-amd64-fips" ]]; then
    echo "ERROR: amd64 FIPS builder does not use an isolated tag" >&2
    exit 1
fi

DOCKER_CALLS=()
docker() {
    DOCKER_CALLS+=("$*")
}

assert_calls_exclude_fips_source() {
    local context=$1
    local calls="${DOCKER_CALLS[*]}"
    if [[ "${calls}" == *"BORING_BSSL_FIPS_SOURCE_PATH="* ]] ||
        [[ "${calls}" == *"edgion-builder:linux-arm64-fips"* ]]; then
        echo "ERROR: ${context} unexpectedly used the FIPS source builder" >&2
        exit 1
    fi
}

assert_calls_include_fips_source() {
    local context=$1
    local calls="${DOCKER_CALLS[*]}"
    if [[ "${calls}" != *"BORING_BSSL_FIPS_SOURCE_PATH=${BORINGSSL_FIPS_SOURCE_PATH}"* ]] ||
        [[ "${calls}" != *"edgion-builder:linux-arm64-fips"* ]]; then
        echo "ERROR: ${context} did not use the pinned FIPS source builder" >&2
        exit 1
    fi
}

BUILDER_IMAGES_READY=""
GATEWAY_FEATURES="default"
GATEWAY_NO_DEFAULT_FEATURES=false
compile_binaries arm64 false >/dev/null
assert_calls_exclude_fips_source "ordinary standard build"

DOCKER_CALLS=()
BUILDER_IMAGES_READY=""
GATEWAY_FEATURES="allocator-jemalloc,fips"
GATEWAY_NO_DEFAULT_FEATURES=true
compile_binaries arm64 false >/dev/null
assert_calls_include_fips_source "explicit FIPS standard build"

DOCKER_CALLS=()
BUILDER_IMAGES_READY=""
compile_controller_gateway_binaries arm64 false >/dev/null
assert_calls_include_fips_source "explicit FIPS controller-gateway build"

DOCKER_CALLS=()
BUILDER_IMAGES_READY=""
compile_test_binaries arm64 false >/dev/null
assert_calls_exclude_fips_source "test-images build"

echo "OK: build-image feature parsing and FIPS builder isolation passed"
