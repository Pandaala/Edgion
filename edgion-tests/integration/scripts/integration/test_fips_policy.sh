#!/usr/bin/env bash
# Integration test for the runtime FIPS-approved TLS policy toggle.
#
# The gateway applies the FIPS-approved cipher/curve set to downstream listeners
# when the policy is active. On a real FIPS build the validated module makes it
# active automatically; on a normal boringssl build the gateway-local
# `fips_policy` config / `EDGION_FIPS_POLICY` env force-enables the same policy
# (gate: fips_module_available() OR fips_policy). Because the policy is pure SSL
# config, this test exercises it on the ordinary boringssl build — no FIPS
# toolchain or validated module required.
#
# It runs an ISOLATED stack (its own minimal Controller + Gateway on dedicated
# ports, EDGION_FIPS_POLICY=1) so it does not disturb the main sweep's single
# shared gateway: the process-global toggle would otherwise force FIPS on every
# listener in the shared run.
#
# Assertions (openssl s_client, TLS 1.2 — BoringSSL cannot restrict TLS 1.3
# suites, so ChaCha20 exclusion is only observable at 1.2):
#   1. A ChaCha20-only client is REJECTED (ChaCha20 is not FIPS-approved).
#   2. An approved AES-GCM client is ACCEPTED (negotiates the approved suite).
#
# Boot model mirrors test_gateway_admin_auth.sh: a minimal file_system Controller
# is the gRPC sync backend; the gateway spawns its data-plane listeners after it
# syncs the seeded Gateway + EdgionTls (with a self-signed cert Secret).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

GATEWAY_BIN="${EDGION_GATEWAY_BIN:-$REPO_ROOT/target/debug/edgion-gateway}"
CONTROLLER_BIN="${EDGION_CONTROLLER_BIN:-$REPO_ROOT/target/debug/edgion-controller}"

TLS_HOST="fips.example.com"

# Ports chosen to avoid the main integration stack (12xxx) and the other 58xxx
# preflight tests (controller-noauth 58200-58213, gateway-admin-auth 58300-58306).
GW_ADMIN_PORT=58310
GW_PROBE_PORT=58311
GW_METRICS_PORT=58312
CTRL_ADMIN_PORT=58313
CTRL_GRPC_PORT=58314
CTRL_PROBE_PORT=58315
GW_HTTPS_PORT=58316

# =============================================================================
# Prerequisites
# =============================================================================
if [ ! -x "$GATEWAY_BIN" ]; then
    echo "ERROR: edgion-gateway binary not found at $GATEWAY_BIN"
    echo "  Run: cargo build --bin edgion-gateway"
    exit 1
fi
if [ ! -x "$CONTROLLER_BIN" ]; then
    echo "ERROR: edgion-controller binary not found at $CONTROLLER_BIN"
    echo "  Run: cargo build --bin edgion-controller"
    exit 1
fi
if ! command -v openssl >/dev/null 2>&1; then
    echo "ERROR: openssl not found (required for the s_client cipher probes)"
    exit 1
fi

# =============================================================================
# Shared helpers (mirrors test_gateway_admin_auth.sh)
# =============================================================================
precheck_port() {
    local port="$1" label="$2"
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "FAIL: port $port ($label) already in use; stop the conflicting process or rerun later"
        lsof -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -3
        return 1
    fi
}

wait_for_tcp_port() {
    local host="$1" port="$2" pid="$3" label="$4" tries="${5:-60}"
    local i
    for i in $(seq 1 "$tries"); do
        if nc -z "$host" "$port" 2>/dev/null; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "FAIL: $label exited during startup"
            return 1
        fi
        sleep 1
    done
    echo "FAIL: $label ${host}:${port} did not accept a connection within ${tries}s"
    return 1
}

wait_for_url_200() {
    local url="$1" pid="$2" label="$3" tries="${4:-60}"
    local i code
    for i in $(seq 1 "$tries"); do
        code=$(curl -s -o /dev/null -w "%{http_code}" "$url" 2>/dev/null || true)
        if [ "$code" = "200" ]; then
            return 0
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "FAIL: $label exited during startup"
            return 1
        fi
        sleep 1
    done
    echo "FAIL: $label $url did not return 200 within ${tries}s"
    return 1
}

# =============================================================================
# Minimal Controller (gRPC sync backend) seeded with the HTTPS listener + cert
# =============================================================================
CTRL_TMPDIR=""
CTRL_PID=""

start_controller() {
    CTRL_TMPDIR=$(mktemp -d)
    local CONFIG_DIR="${CTRL_TMPDIR}/config"
    local LOG_DIR="${CTRL_TMPDIR}/logs"
    local CONF_DIR="${CTRL_TMPDIR}/conf"
    local CERT_DIR="${CTRL_TMPDIR}/certs"
    mkdir -p "$CONFIG_DIR" "$LOG_DIR" "$CONF_DIR" "$CERT_DIR"

    if [ -d "${REPO_ROOT}/config/crd" ]; then
        cp -R "${REPO_ROOT}/config/crd/." "${CONFIG_DIR}/crd/"
    else
        echo "FAIL: CRD schema source not found: ${REPO_ROOT}/config/crd"
        return 1
    fi

    # Self-signed RSA cert (CN + SAN = TLS_HOST) via a portable openssl config
    # (works on both OpenSSL and LibreSSL). RSA so the approved ECDHE-RSA-AES-GCM
    # suite negotiates.
    cat > "${CERT_DIR}/openssl.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = ${TLS_HOST}
[ext]
subjectAltName = DNS:${TLS_HOST}
EOF
    if ! openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 2 \
            -keyout "${CERT_DIR}/tls.key" -out "${CERT_DIR}/tls.crt" \
            -config "${CERT_DIR}/openssl.cnf" >/dev/null 2>&1; then
        echo "FAIL: openssl self-signed cert generation failed"
        return 1
    fi
    local CRT_B64 KEY_B64
    CRT_B64=$(base64 < "${CERT_DIR}/tls.crt" | tr -d '\n')
    KEY_B64=$(base64 < "${CERT_DIR}/tls.key" | tr -d '\n')

    # Seed cluster-scoped base resources + the namespaced HTTPS topology.
    mkdir -p "${CONF_DIR}/cluster" "${CONF_DIR}/edgion-test"
    local base="${REPO_ROOT}/edgion-tests/integration/conf/base"
    if [ ! -f "${base}/GatewayClass.yaml" ] || [ ! -f "${base}/EdgionGatewayConfig.yaml" ]; then
        echo "FAIL: base Gateway resources not found under ${base}"
        return 1
    fi
    cp "${base}/GatewayClass.yaml" "${CONF_DIR}/cluster/GatewayClass__public-gateway.yaml"
    cp "${base}/EdgionGatewayConfig.yaml" "${CONF_DIR}/cluster/EdgionGatewayConfig__example-gateway.yaml"

    cat > "${CONF_DIR}/edgion-test/Gateway_edgion-test_fips-gw.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: fips-gw
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: https
      protocol: HTTPS
      port: ${GW_HTTPS_PORT}
      hostname: ${TLS_HOST}
      allowedRoutes:
        namespaces:
          from: All
EOF

    cat > "${CONF_DIR}/edgion-test/Secret_edgion-test_fips-cert.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: fips-cert
  namespace: edgion-test
type: kubernetes.io/tls
data:
  tls.crt: ${CRT_B64}
  tls.key: ${KEY_B64}
EOF

    # EdgionTls WITHOUT spec.ciphers: on a FIPS-active gateway this listener
    # therefore gets the approved default cipher list + curves (the code path
    # under test). An explicit spec.ciphers would instead take the operator
    # override branch.
    cat > "${CONF_DIR}/edgion-test/EdgionTls_edgion-test_fips-tls.yaml" <<EOF
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: fips-tls
  namespace: edgion-test
spec:
  parentRefs:
    - name: fips-gw
      namespace: edgion-test
      sectionName: https
  hosts:
    - ${TLS_HOST}
  secretRef:
    name: fips-cert
    namespace: edgion-test
  minTlsVersion: TLS1_2
EOF

    cat > "$CTRL_TMPDIR/controller.yaml" <<EOF
conf_center:
  type: file_system
  conf_dir: ${CONF_DIR}
EOF

    precheck_port "$CTRL_ADMIN_PORT" "controller admin" || return 1
    precheck_port "$CTRL_GRPC_PORT"  "controller gRPC"  || return 1
    precheck_port "$CTRL_PROBE_PORT" "controller probe" || return 1

    echo "=== Starting backend Controller (gRPC sync source) ==="
    "$CONTROLLER_BIN" \
        -c "$CTRL_TMPDIR/controller.yaml" \
        --work-dir "$CTRL_TMPDIR" \
        --log-dir "$LOG_DIR" \
        --admin-listen "127.0.0.1:${CTRL_ADMIN_PORT}" \
        --grpc-listen "127.0.0.1:${CTRL_GRPC_PORT}" \
        --probe-listen "127.0.0.1:${CTRL_PROBE_PORT}" \
        > "$LOG_DIR/controller.log" 2>&1 &
    CTRL_PID=$!

    if ! wait_for_url_200 \
            "http://127.0.0.1:${CTRL_PROBE_PORT}/ready" \
            "$CTRL_PID" \
            "controller /ready" 90; then
        cat "$LOG_DIR/controller.log"
        return 1
    fi
    echo "PASS: backend Controller is ready (gRPC on 127.0.0.1:${CTRL_GRPC_PORT})"
}

# =============================================================================
# Gateway launch (EDGION_FIPS_POLICY=1)
# =============================================================================
GW_TMPDIR=""
GW_PID=""
GW_LOG=""

start_gateway() {
    GW_TMPDIR=$(mktemp -d)
    local LOG_DIR="${GW_TMPDIR}/logs"
    mkdir -p "$LOG_DIR"
    GW_LOG="$LOG_DIR/gateway.log"

    cat > "$GW_TMPDIR/gateway.yaml" <<EOF
work_dir: "."
logging:
  log_dir: "${LOG_DIR}"
metrics:
  listen: "127.0.0.1:${GW_METRICS_PORT}"
EOF

    precheck_port "$GW_ADMIN_PORT"   "gateway admin"   || return 1
    precheck_port "$GW_PROBE_PORT"   "gateway probe"   || return 1
    precheck_port "$GW_METRICS_PORT" "gateway metrics" || return 1
    precheck_port "$GW_HTTPS_PORT"   "gateway https"   || return 1

    echo "=== Starting gateway with EDGION_FIPS_POLICY=1 ==="
    RUST_LOG="info,pingora_proxy=error,pingora_core=error" \
    EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
    EDGION_FIPS_POLICY=1 \
    "$GATEWAY_BIN" \
        -c "$GW_TMPDIR/gateway.yaml" \
        --work-dir "$GW_TMPDIR" \
        --server-addr "http://127.0.0.1:${CTRL_GRPC_PORT}" \
        --integration-testing-mode \
        --admin-listen "127.0.0.1:${GW_ADMIN_PORT}" \
        --probe-listen "127.0.0.1:${GW_PROBE_PORT}" \
        > "$GW_LOG" 2>&1 &
    GW_PID=$!

    if ! wait_for_url_200 \
            "http://127.0.0.1:${GW_PROBE_PORT}/health" \
            "$GW_PID" \
            "gateway (/health)" 60; then
        echo "---- gateway log ----"
        cat "$GW_LOG"
        return 1
    fi

    # Data-plane HTTPS listener binds after the Gateway/EdgionTls sync.
    if ! wait_for_tcp_port "127.0.0.1" "$GW_HTTPS_PORT" "$GW_PID" "gateway HTTPS listener" 60; then
        echo "---- gateway log ----"
        cat "$GW_LOG"
        return 1
    fi
    echo "PASS: gateway HTTPS listener is accepting connections on ${GW_HTTPS_PORT}"
}

# =============================================================================
# Cleanup
# =============================================================================
cleanup() {
    [ -n "$GW_PID" ] && kill "$GW_PID" 2>/dev/null || true
    [ -n "$GW_PID" ] && wait "$GW_PID" 2>/dev/null || true
    [ -n "$GW_TMPDIR" ] && rm -rf "$GW_TMPDIR" || true
    [ -n "$CTRL_PID" ] && kill "$CTRL_PID" 2>/dev/null || true
    [ -n "$CTRL_PID" ] && wait "$CTRL_PID" 2>/dev/null || true
    [ -n "$CTRL_TMPDIR" ] && rm -rf "$CTRL_TMPDIR" || true
}
trap cleanup EXIT

# One TLS 1.2 handshake attempt with the given client cipher list; prints the
# openssl s_client output.
probe_cipher() {
    local ciphers="$1"
    echo | openssl s_client \
        -connect "127.0.0.1:${GW_HTTPS_PORT}" \
        -servername "${TLS_HOST}" \
        -tls1_2 \
        -cipher "$ciphers" 2>&1 || true
}

# =============================================================================
# Assertions
# =============================================================================
run_assertions() {
    echo ""
    echo "=== [1/2] ChaCha20-only client must be REJECTED under FIPS policy ==="
    local out
    out=$(probe_cipher "ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-CHACHA20-POLY1305")
    if echo "$out" | grep -qi "Cipher is ECDHE-.*CHACHA20"; then
        echo "FAIL [1/2]: ChaCha20 was negotiated — FIPS policy did not reject it"
        echo "$out" | grep -i "Cipher is" || true
        return 1
    fi
    if ! echo "$out" | grep -qiE "handshake failure|no cipher|Cipher is \(NONE\)|ssl handshake failure|sslv3 alert"; then
        echo "FAIL [1/2]: ChaCha20 handshake did not show a clear rejection; output:"
        echo "$out" | tail -20
        return 1
    fi
    echo "PASS [1/2]: ChaCha20-only client rejected"

    echo ""
    echo "=== [2/2] Approved AES-GCM client must be ACCEPTED ==="
    out=$(probe_cipher "ECDHE-RSA-AES256-GCM-SHA384")
    if ! echo "$out" | grep -q "Cipher is ECDHE-RSA-AES256-GCM-SHA384"; then
        echo "FAIL [2/2]: approved suite did not negotiate; output:"
        echo "$out" | tail -20
        return 1
    fi
    echo "PASS [2/2]: approved suite ECDHE-RSA-AES256-GCM-SHA384 negotiated"
}

# =============================================================================
# Entry point
# =============================================================================
main() {
    start_controller
    start_gateway
    run_assertions
    echo ""
    echo "PASS: FIPS policy (ChaCha20 rejected / approved suite accepted)"
}

main
