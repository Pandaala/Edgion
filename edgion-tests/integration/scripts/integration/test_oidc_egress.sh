#!/usr/bin/env bash
# D0057 standalone topology exception: deterministic multi-address DNS and
# proxy-environment injection must exist when the Gateway process constructs
# its OpenID Connect clients. The shared Gateway cannot adopt those process-
# local settings. This script starts exactly one Controller and one Gateway,
# updates only its own resources online, and leaves existing OIDC suites alone.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
source "${SCRIPT_DIR}/../utils/shell_compat.sh"
source "${SCRIPT_DIR}/../utils/conf_sync_certs.sh"

BIN_DIR="$(dirname "${EDGION_GATEWAY_BIN:-${REPO_ROOT}/target/debug/edgion-gateway}")"
if [ "${1:-}" = "--build-resolver-only" ]; then
    BIN_DIR="${2:-$BIN_DIR}"
    mkdir -p "$BIN_DIR"
    case "$EDGION_SHELL_PLATFORM" in
        darwin)
            cc -Wall -Wextra -Werror -dynamiclib \
                "${SCRIPT_DIR}/oidc_egress/resolver_fixture.c" \
                -o "${BIN_DIR}/oidc_egress_resolver.dylib"
            ;;
        linux)
            cc -Wall -Wextra -Werror -shared -fPIC \
                "${SCRIPT_DIR}/oidc_egress/resolver_fixture.c" -ldl \
                -o "${BIN_DIR}/oidc_egress_resolver.so"
            ;;
        *)
            echo "ERROR: unsupported resolver fixture platform" >&2
            exit 1
            ;;
    esac
    exit 0
fi
if [ "$#" -ne 0 ]; then
    echo "Usage: $0 [--build-resolver-only [binary-directory]]" >&2
    exit 2
fi

case "$EDGION_SHELL_PLATFORM" in
    darwin) RESOLVER_LIBRARY="${BIN_DIR}/oidc_egress_resolver.dylib" ;;
    linux) RESOLVER_LIBRARY="${BIN_DIR}/oidc_egress_resolver.so" ;;
    *) echo "ERROR: unsupported resolver fixture platform" >&2; exit 1 ;;
esac
if [ ! -f "$RESOLVER_LIBRARY" ]; then
    echo "ERROR: missing $RESOLVER_LIBRARY; run prepare.sh first" >&2
    exit 1
fi

for binary in edgion-controller edgion-gateway edgion-cli; do
    if [ ! -x "${BIN_DIR}/${binary}" ]; then
        echo "ERROR: missing ${BIN_DIR}/${binary}; run prepare.sh first" >&2
        exit 1
    fi
done

mkdir -p "${REPO_ROOT}/tmp/integration_testing"
WORK_DIR=$(mktemp -d "${REPO_ROOT}/tmp/integration_testing/oidc_egress.XXXXXX")
echo "OIDC egress artifacts: ${WORK_DIR}"
conf_sync_distribute_certs "$WORK_DIR" > "${WORK_DIR}/conf_sync_certs.log" 2>&1
conf_sync_security_block server > "${WORK_DIR}/server_security.yaml"
conf_sync_security_block client > "${WORK_DIR}/client_security.yaml"

exec python3 "${SCRIPT_DIR}/oidc_egress/scenario.py" \
    "$REPO_ROOT" "$WORK_DIR" "$BIN_DIR" "$RESOLVER_LIBRARY"
