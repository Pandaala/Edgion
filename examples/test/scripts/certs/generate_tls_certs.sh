#!/bin/bash
# =============================================================================
# TLS Certificate Generation Script for Edgion Gateway Testing
# - Cert files are written under examples/test/certs
# - Secret YAML is written to runtime directory only (never conf/)
# =============================================================================

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[OK]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERR]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

TEMP_DIR="/tmp/edgion-certs-$$"
mkdir -p "$TEMP_DIR"
trap "rm -rf $TEMP_DIR" EXIT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CERTS_DIR="$PROJECT_ROOT/examples/test/certs"
SECRET_OUTPUT_ROOT="${EDGION_GENERATED_SECRET_DIR:-$PROJECT_ROOT/integration_testing/generated_secrets_manual}"
SECRET_OUTPUT_DIR="$SECRET_OUTPUT_ROOT/base"
SECRET_OUTPUT_FILE="$SECRET_OUTPUT_DIR/Secret_edgion-test_edge-tls.yaml"

mkdir -p "$CERTS_DIR"
mkdir -p "$SECRET_OUTPUT_DIR"

create_secret_yaml() {
    local name=$1
    local namespace=$2
    local cert_file=$3
    local key_file=$4
    local output_file=$5
    local cert_b64
    local key_b64

    cert_b64=$(base64 < "$cert_file" | tr -d '\n')
    key_b64=$(base64 < "$key_file" | tr -d '\n')

    cat > "$output_file" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: $name
  namespace: $namespace
type: kubernetes.io/tls
data:
  tls.crt: $cert_b64
  tls.key: $key_b64
EOF
}

generate_tls_cert() {
    log_info "Generating multi-SAN TLS certificate..."
    openssl req -x509 -newkey rsa:2048 -nodes \
      -keyout "$TEMP_DIR/edge-tls.key" \
      -out "$TEMP_DIR/edge-tls.crt" \
      -days 365 \
      -subj "/CN=test.example.com" \
      -addext "subjectAltName=DNS:test.example.com,DNS:grpc.example.com,DNS:tcp.example.com,DNS:match-test.example.com,DNS:*.wildcard.example.com,DNS:section-test.example.com,DNS:gateway-tls.test.com" \
      2>/dev/null

    cp "$TEMP_DIR/edge-tls.crt" "$CERTS_DIR/server.crt"
    cp "$TEMP_DIR/edge-tls.key" "$CERTS_DIR/server.key"
    cp "$TEMP_DIR/edge-tls.crt" "$CERTS_DIR/ca.pem"
}

log_section "Generate TLS certificate"
log_info "temp directory: $TEMP_DIR"
log_info "cert directory: $CERTS_DIR"
log_info "runtime secret output: $SECRET_OUTPUT_FILE"

if [ -f "$CERTS_DIR/server.crt" ] && [ -f "$CERTS_DIR/server.key" ]; then
    log_info "Found existing TLS cert/key, reusing them."
else
    generate_tls_cert
    log_success "TLS cert/key generated."
fi

if [ ! -f "$CERTS_DIR/ca.pem" ] && [ -f "$CERTS_DIR/server.crt" ]; then
    cp "$CERTS_DIR/server.crt" "$CERTS_DIR/ca.pem"
fi

create_secret_yaml \
    "edge-tls" \
    "edgion-test" \
    "$CERTS_DIR/server.crt" \
    "$CERTS_DIR/server.key" \
    "$SECRET_OUTPUT_FILE"

log_section "Completed"
log_success "Runtime TLS Secret generated."
log_info "Secret YAML: $SECRET_OUTPUT_FILE"
log_info "Test cert files: $CERTS_DIR/ca.pem, $CERTS_DIR/server.crt, $CERTS_DIR/server.key"
