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
if [[ "$(builder_image_tag linux/amd64)" != "edgion-builder:linux-amd64" ]]; then
    echo "ERROR: amd64 builder tag changed unexpectedly" >&2
    exit 1
fi

DOCKER_CALLS=()
DOCKER_JOB_VALUES=()
docker() {
    DOCKER_CALLS+=("$*")
    DOCKER_JOB_VALUES+=("${CARGO_BUILD_JOBS-unset}")
}

assert_calls() {
    local context=$1 expected_fips=$2 expected_jobs=$3
    local calls="${DOCKER_CALLS[*]}"
    if [[ "${calls}" == *"BORING_BSSL_FIPS_SOURCE_PATH="* ]] ||
        [[ "${calls}" == *"INCLUDE_BORINGSSL_FIPS_SOURCE="* ]] ||
        [[ "${calls}" == *"BORINGSSL_FIPS_"* ]] ||
        [[ "${calls}" == *"edgion-builder:linux-arm64-fips"* ]]; then
        echo "ERROR: ${context} used the obsolete FIPS source override" >&2
        exit 1
    fi
    if [[ "${calls}" != *"edgion-builder:linux-arm64"* ]] ||
        [[ "${calls}" != *"-e CARGO_BUILD_JOBS "* ]]; then
        echo "ERROR: ${context} did not use the shared builder and optional job cap" >&2
        exit 1
    fi
    local actual_fips=false
    if [[ "${calls}" == *"EDGION_EXPECT_FIPS_MODULE=1"* ]]; then
        actual_fips=true
    fi
    if [[ "${actual_fips}" != "${expected_fips}" ]]; then
        echo "ERROR: ${context} selected the wrong runtime verification mode" >&2
        exit 1
    fi
    local jobs
    for jobs in "${DOCKER_JOB_VALUES[@]}"; do
        if [[ "${jobs}" != "${expected_jobs}" ]]; then
            echo "ERROR: ${context} changed the optional host job cap" >&2
            exit 1
        fi
    done
}

reset_calls() {
    DOCKER_CALLS=()
    DOCKER_JOB_VALUES=()
    BUILDER_IMAGES_READY=""
}

unset CARGO_BUILD_JOBS
reset_calls
GATEWAY_FEATURES="default"
GATEWAY_NO_DEFAULT_FEATURES=false
compile_binaries arm64 false >/dev/null
assert_calls "ordinary standard build" false unset

export CARGO_BUILD_JOBS=2
reset_calls
GATEWAY_FEATURES="allocator-jemalloc,fips"
GATEWAY_NO_DEFAULT_FEATURES=true
compile_binaries arm64 false >/dev/null
assert_calls "explicit FIPS standard build" true 2

reset_calls
compile_controller_gateway_binaries arm64 false >/dev/null
assert_calls "explicit FIPS controller-gateway build" true 2

reset_calls
compile_test_binaries arm64 false >/dev/null
assert_calls "test-images build" false 2

unset CARGO_BUILD_JOBS
reset_calls
GATEWAY_FEATURES="default"
GATEWAY_NO_DEFAULT_FEATURES=false
compile_controller_gateway_binaries arm64 false >/dev/null
assert_calls "ordinary controller-gateway build" false unset

echo "OK: exact FIPS selection, shared builders, optional jobs and runtime verification passed"
