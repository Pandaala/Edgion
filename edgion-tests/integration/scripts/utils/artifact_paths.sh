#!/bin/bash
# Canonical integration artifact selection; sourcing has no side effects.
edgion_resolve_artifacts() {
    EDGION_TEST_PROFILE="${EDGION_TEST_PROFILE:-debug}"
    case "$EDGION_TEST_PROFILE" in
        debug|release) ;;
        *) echo "Unsupported integration profile: $EDGION_TEST_PROFILE" >&2; return 2 ;;
    esac
    if [ "${EDGION_COVERAGE:-0}" = 1 ]; then
        if [ "$EDGION_TEST_PROFILE" != debug ]; then
            echo "Coverage requires the debug profile" >&2
            return 2
        fi
        EDGION_TEST_TARGET_DIR="${PROJECT_ROOT}/target/llvm-cov-target"
    else
        EDGION_TEST_TARGET_DIR="${PROJECT_ROOT}/target"
    fi
    EDGION_TEST_BIN_DIR="${EDGION_TEST_TARGET_DIR}/${EDGION_TEST_PROFILE}"
    export EDGION_TEST_PROFILE EDGION_TEST_TARGET_DIR EDGION_TEST_BIN_DIR
}

edgion_validate_owned_work_dir() {
    [ -n "${1:-}" ] && [ -d "$1" ] || return 2
    local canonical parent
    canonical=$(cd "$1" && pwd -P) || return 2
    parent=$(cd "${PROJECT_ROOT}/tmp/integration_testing" && pwd -P) || return 2
    case "$canonical" in
        "$parent"/ai-performance.*) ;;
        *) echo "Invalid owned performance work directory" >&2; return 2 ;;
    esac
    [ "$canonical" = "$1" ] && [ "$(dirname "$canonical")" = "$parent" ] || return 2
}

edgion_refuse_existing_topology() {
    # Read-only preflight. Never signal a process discovered by name.
    if ps -axo command= | awk -v root="${PROJECT_ROOT}/target/" '
        index($0, root) == 1 && $0 ~ /\/(edgion-gateway|edgion-controller|test_server)([[:space:]]|$)/ { found=1 }
        END { exit !found }'; then
        echo "Existing Edgion integration topology; refusing release startup" >&2
        return 2
    fi
}
