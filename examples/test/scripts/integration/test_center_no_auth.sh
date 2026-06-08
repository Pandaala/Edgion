#!/usr/bin/env bash
# Integration test for Center starting up without authentication configuration.
#
# Verifies that, when the Center has neither [local_auth] nor [auth] in TOML/YAML:
# 1. Startup succeeds (no panic, no exit)
# 2. /health returns 200
# 3. /api/v1/auth/status returns 200
# 4. /api/v1/controllers (or any business path) without a token returns 503
#    (middleware fail-close branch: require_auth=true + no provider ready -> 503)
# 5. Startup log contains the WARN "No authentication configured"
#

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

TMPDIR=$(mktemp -d)

# Generate a CA + center server cert so the center can start with real mTLS.
# No controller connects in this test; the certs only need to allow startup.
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TMPDIR/ca.key" \
  -out    "$TMPDIR/ca.crt" \
  -days   30 \
  -subj   "/CN=Edgion No-Auth Test CA/O=EdgionTest" \
  2>/dev/null

openssl req -newkey rsa:2048 -nodes \
  -keyout "$TMPDIR/server.key" \
  -out    "$TMPDIR/server.csr" \
  -subj   "/CN=edgion-center/O=EdgionTest" \
  2>/dev/null

cat > "$TMPDIR/server.ext" <<EOF
subjectAltName=IP:127.0.0.1,DNS:localhost
extendedKeyUsage=serverAuth
EOF

openssl x509 -req \
  -in     "$TMPDIR/server.csr" \
  -CA     "$TMPDIR/ca.crt" \
  -CAkey  "$TMPDIR/ca.key" \
  -CAcreateserial \
  -out    "$TMPDIR/server.crt" \
  -days   30 \
  -extfile "$TMPDIR/server.ext" \
  2>/dev/null

# Minimal Center configuration, no auth / local_auth section
cat > "$TMPDIR/center.yaml" <<EOF
server:
  http_addr: "127.0.0.1:58100"
  grpc_addr: "127.0.0.1:58110"
  probe_addr: "127.0.0.1:58101"
  metrics_addr: "127.0.0.1:58109"
database:
  enabled: false
sync:
  command_timeout_secs: 30
grpc_security:
  active: fed
  certs:
    - name: fed
      cert: $TMPDIR/server.crt
      key: $TMPDIR/server.key
      ca: $TMPDIR/ca.crt
peer_identity:
  trust_domain: "edgion.io"
EOF

cd "$REPO_ROOT"

# Port precheck: avoid unclear startup errors caused by leftover processes or parallel CI holding the port.
for port in 58100 58101 58109 58110; do
    if lsof -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "FAIL: port $port already in use; stop the conflicting process or rerun later"
        lsof -iTCP:"$port" -sTCP:LISTEN 2>/dev/null | head -3
        exit 1
    fi
done

# Make sure edgion-center has been built
if [ ! -x "$REPO_ROOT/target/debug/edgion-center" ]; then
    echo "Building edgion-center..."
    cargo build --bin edgion-center
fi

"$REPO_ROOT/target/debug/edgion-center" -c "$TMPDIR/center.yaml" > "$TMPDIR/center.log" 2>&1 &
CENTER_PID=$!

cleanup() {
    kill "$CENTER_PID" 2>/dev/null || true
    wait "$CENTER_PID" 2>/dev/null || true
    rm -rf "$TMPDIR"
}
trap cleanup EXIT

# Wait for the HTTP port to become ready
READY=false
for _ in $(seq 1 20); do
    if curl -sf http://127.0.0.1:58101/health > /dev/null 2>&1; then
        READY=true
        break
    fi
    if ! kill -0 "$CENTER_PID" 2>/dev/null; then
        echo "FAIL: center exited during startup"
        cat "$TMPDIR/center.log"
        exit 1
    fi
    sleep 1
done

if ! $READY; then
    echo "FAIL: center HTTP /health did not become ready within 20s"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[1/4] Testing /health..."
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:58101/health)
if [ "$STATUS" != "200" ]; then
    echo "FAIL: /health returned $STATUS (expected 200)"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[2/4] Testing /api/v1/auth/status..."
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:58100/api/v1/auth/status)
if [ "$STATUS" != "200" ]; then
    echo "FAIL: /auth/status returned $STATUS (expected 200)"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[3/4] Testing business route returns 503 (fail-close via 503)..."
# Center business path: /api/v1/controllers without a token should return 503 (not 200, not 401)
STATUS=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:58100/api/v1/controllers)
if [ "$STATUS" != "503" ]; then
    echo "FAIL: business route /api/v1/controllers returned $STATUS (expected 503 fail-close)"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "[4/4] Checking startup WARN log..."
if ! grep -q "No authentication configured" "$TMPDIR/center.log"; then
    echo "FAIL: Expected WARN log 'No authentication configured' not found"
    cat "$TMPDIR/center.log"
    exit 1
fi

echo "PASS: Center no-auth startup scenario"
