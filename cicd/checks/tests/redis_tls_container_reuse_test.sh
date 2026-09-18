#!/usr/bin/env bash
# Redis TLS container reuse must follow the current run's certificate material.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../.." && pwd)"
RUNNER="${PROJECT_ROOT}/edgion-tests/integration/scripts/integration/run_integration.sh"
CERT_SCRIPT="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/redis/make-test-certs.sh"
REDIS_COMPOSE_DIR="${PROJECT_ROOT}/edgion-tests/integration/conf/Services/redis"
REDIS_CERTS_DIR="${REDIS_COMPOSE_DIR}/certs"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

extract_function() {
    local name="$1"
    local body
    body=$(sed -n "/^${name}()/,/^}/p" "$RUNNER")
    [ -n "$body" ] || fail "function ${name} not found"
    printf '%s\n' "$body"
}

bash -n "$RUNNER" "$CERT_SCRIPT"

probe_src=$(extract_function probe_redis_tls_ready)
if printf '%s\n' "$probe_src" | grep -q 'docker exec'; then
    fail "probe_redis_tls_ready still treats container-local certs as reuse evidence"
fi
printf '%s\n' "$probe_src" | grep -q 'probe_redis_tls_identity' \
    || fail "probe_redis_tls_ready does not require host-side certificate identity"

identity_src=$(extract_function probe_redis_tls_identity)
if printf '%s\n' "$identity_src" | grep -q 'docker exec'; then
    fail "probe_redis_tls_identity still uses container-local certs"
fi
printf '%s\n' "$identity_src" | grep -q -- '-CAfile' \
    || fail "probe_redis_tls_identity does not verify the live server with the current CA"
printf '%s\n' "$identity_src" | grep -F -q "grep -F -q 'Verify return code: 0 (ok)'" \
    || fail "probe_redis_tls_identity does not fail closed on the Verify return code line"

start_src=$(extract_function start_redis_service_if_needed)
printf '%s\n' "$start_src" | grep -q -- '--force-recreate' \
    || fail "TLS Redis containers can be left running with stale in-memory certificates"

echo "PASS: Redis TLS reuse is coupled to current-run certificate identity"

docker_live_ready() {
    command -v docker >/dev/null 2>&1 \
        && docker info >/dev/null 2>&1 \
        && docker compose version >/dev/null 2>&1 \
        && command -v openssl >/dev/null 2>&1
}

if ! docker_live_ready; then
    echo "SKIP: consecutive work-directory handshake proof requires Docker and openssl"
    exit 0
fi

# shellcheck source=../../../edgion-tests/integration/scripts/integration/run_integration.sh
source "$RUNNER"

write_secret_file() {
    local yaml="$1"
    local key="$2"
    local dest="$3"
    python3 -c '
import base64
import pathlib
import sys

text = pathlib.Path(sys.argv[1]).read_text()
key = sys.argv[2] + ":"
for line in text.splitlines():
    stripped = line.strip()
    if stripped.startswith(key):
        value = stripped.split(":", 1)[1].strip()
        pathlib.Path(sys.argv[3]).write_bytes(base64.b64decode(value))
        raise SystemExit(0)
raise SystemExit(1)
' "$yaml" "$key" "$dest"
}

fingerprint_cert() {
    openssl x509 -in "$1" -noout -fingerprint -sha256
}

handshake_tls() {
    local ca="$1"
    local output
    output=$(printf 'Q\n' | openssl s_client \
        -connect "${REDIS_HOST}:${REDIS_TLS_PORT}" \
        -CAfile "$ca" \
        -verify_return_error 2>&1) || return 1
    printf '%s\n' "$output" | grep -F -q 'Verify return code: 0 (ok)'
}

handshake_mtls() {
    local ca="$1"
    local cert="$2"
    local key="$3"
    local output
    output=$(printf 'Q\n' | openssl s_client \
        -connect "${REDIS_HOST}:${REDIS_MTLS_PORT}" \
        -CAfile "$ca" \
        -cert "$cert" \
        -key "$key" \
        -verify_return_error 2>&1) || return 1
    printf '%s\n' "$output" | grep -F -q 'Verify return code: 0 (ok)'
}

prove_three_clients() {
    local work_dir="$1"
    local secret_dir="${work_dir}/LinkSys/RedisTls"
    local unpacked="${work_dir}/unpacked"
    mkdir -p "$unpacked"

    [ -f "${secret_dir}/Secret_redis-tls-ca.yaml" ] \
        || fail "work directory missing redis-tls-ca Secret: ${work_dir}"
    [ -f "${secret_dir}/Secret_redis-tls-client.yaml" ] \
        || fail "work directory missing redis-tls-client Secret: ${work_dir}"

    write_secret_file "${secret_dir}/Secret_redis-tls-ca.yaml" "ca.crt" "${unpacked}/ca.crt"
    write_secret_file "${secret_dir}/Secret_redis-tls-client.yaml" "tls.crt" "${unpacked}/client.crt"
    write_secret_file "${secret_dir}/Secret_redis-tls-client.yaml" "tls.key" "${unpacked}/client.key"

    [ "$(fingerprint_cert "${unpacked}/ca.crt")" = "$(fingerprint_cert "${REDIS_CERTS_DIR}/ca.crt")" ] \
        || fail "generated Secret CA drifted from the current host certificate set"
    [ "$(fingerprint_cert "${unpacked}/client.crt")" = "$(fingerprint_cert "${REDIS_CERTS_DIR}/client.crt")" ] \
        || fail "generated Secret client certificate drifted from the current host certificate set"

    handshake_tls "${REDIS_CERTS_DIR}/ca.crt" \
        || fail "scheme-only default-trust handshake failed for ${work_dir}"
    handshake_tls "${unpacked}/ca.crt" \
        || fail "custom-CA handshake failed for ${work_dir}"
    handshake_mtls "${unpacked}/ca.crt" "${unpacked}/client.crt" "${unpacked}/client.key" \
        || fail "mTLS handshake failed for ${work_dir}"
}

container_id() {
    docker inspect -f '{{.Id}}' "$1" 2>/dev/null || true
}

test_tmp=$(mktemp -d)
cleanup_live() {
    rm -rf "$test_tmp"
}
trap cleanup_live EXIT

REDIS_SERVICE_REQUIRED=true
REDIS_TLS_SERVICE_REQUIRED=true
REDIS_SERVICE_STARTED=false
REDIS_STARTED_SERVICES=()

workdir1="${test_tmp}/testing_first"
workdir2="${test_tmp}/testing_second"
workdir3="${test_tmp}/testing_third"

EDGION_GENERATED_SECRET_DIR="$workdir1" bash "$CERT_SCRIPT" \
    || fail "first work-directory certificate preparation failed"
start_redis_service_if_needed \
    || fail "Redis TLS services did not become ready for the first work directory"
tls_id_before=$(container_id "$REDIS_TLS_CONTAINER_NAME")
mtls_id_before=$(container_id "$REDIS_MTLS_CONTAINER_NAME")
[ -n "$tls_id_before" ] && [ -n "$mtls_id_before" ] \
    || fail "expected Redis TLS containers to exist after the first work directory"
prove_three_clients "$workdir1"

EDGION_GENERATED_SECRET_DIR="$workdir2" bash "$CERT_SCRIPT" \
    || fail "second work-directory certificate preparation failed"
start_redis_service_if_needed \
    || fail "compatible Redis TLS containers were not reused for the second work directory"
[ "$(container_id "$REDIS_TLS_CONTAINER_NAME")" = "$tls_id_before" ] \
    || fail "compatible redis-tls container was recreated instead of reused"
[ "$(container_id "$REDIS_MTLS_CONTAINER_NAME")" = "$mtls_id_before" ] \
    || fail "compatible redis-mtls container was recreated instead of reused"
prove_three_clients "$workdir2"

rm -f "${REDIS_CERTS_DIR}/ca.crt" "${REDIS_CERTS_DIR}/server.crt" \
    "${REDIS_CERTS_DIR}/server.key" "${REDIS_CERTS_DIR}/client.crt" \
    "${REDIS_CERTS_DIR}/client.key"
EDGION_GENERATED_SECRET_DIR="$workdir3" bash "$CERT_SCRIPT" \
    || fail "third work-directory certificate regeneration failed"
if probe_redis_tls_ready "$REDIS_HOST" "$REDIS_TLS_PORT" "$REDIS_PASSWORD" false \
    || probe_redis_tls_ready "$REDIS_HOST" "$REDIS_MTLS_PORT" "$REDIS_PASSWORD" true; then
    fail "stale Redis TLS containers still satisfied the current-run certificate identity"
fi
start_redis_service_if_needed \
    || fail "Redis TLS services were not recreated after certificate drift"
[ "$(container_id "$REDIS_TLS_CONTAINER_NAME")" != "$tls_id_before" ] \
    || fail "redis-tls container retained stale certificates after regeneration"
[ "$(container_id "$REDIS_MTLS_CONTAINER_NAME")" != "$mtls_id_before" ] \
    || fail "redis-mtls container retained stale certificates after regeneration"
prove_three_clients "$workdir3"

echo "PASS: consecutive work directories keep all three secure Redis handshakes ready"
