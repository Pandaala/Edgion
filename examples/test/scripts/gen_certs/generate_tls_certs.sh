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
STATUS_REFGRANT_SECRET_DIR="$SECRET_OUTPUT_ROOT/Status/RefGrant"

mkdir -p "$CERTS_DIR"
mkdir -p "$SECRET_OUTPUT_DIR"
mkdir -p "$STATUS_REFGRANT_SECRET_DIR"

REQUIRED_SAN_ENTRIES=(
    "DNS:test.example.com"
    "DNS:grpc.example.com"
    "DNS:tcp.example.com"
    "DNS:match-test.example.com"
    "DNS:*.wildcard.example.com"
    "DNS:section-test.example.com"
    "DNS:default.example.com"
    "DNS:gateway-tls.test.com"
    "DNS:*.sandbox.example.com"
    "DNS:*.pp2.example.com"
    "DNS:*.sp-allow.example.com"
    "DNS:*.sp-deny.example.com"
    "DNS:*.both-absent.example.com"
    "DNS:both-absent-tls.example.com"
    "DNS:port-only.example.com"
    "DNS:no-hostname-gateway-tls.test.com"
    "DNS:sni421-exact.test.com"
    "DNS:*.sni421.test.com"
    "DNS:*.example.com"
)

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

write_status_refgrant_tls_secrets() {
    local cert_file=$1
    local key_file=$2

    create_secret_yaml \
        "refgrant-test-cert" \
        "cert-store" \
        "$cert_file" \
        "$key_file" \
        "$STATUS_REFGRANT_SECRET_DIR/Secret_cert-store_refgrant-test-cert.yaml"

    create_secret_yaml \
        "refgrant-test-cert" \
        "backend" \
        "$cert_file" \
        "$key_file" \
        "$STATUS_REFGRANT_SECRET_DIR/Secret_backend_refgrant-test-cert.yaml"

    create_secret_yaml \
        "other-cert" \
        "backend" \
        "$cert_file" \
        "$key_file" \
        "$STATUS_REFGRANT_SECRET_DIR/Secret_backend_other-cert.yaml"

    create_secret_yaml \
        "no-grant-cert" \
        "backend" \
        "$cert_file" \
        "$key_file" \
        "$STATUS_REFGRANT_SECRET_DIR/Secret_backend_no-grant-cert.yaml"
}

generate_tls_cert() {
    log_info "Generating multi-SAN TLS certificate..."
    openssl req -x509 -newkey rsa:2048 -nodes \
      -keyout "$TEMP_DIR/edge-tls.key" \
      -out "$TEMP_DIR/edge-tls.crt" \
      -days 365 \
      -subj "/CN=test.example.com" \
      -addext "subjectAltName=DNS:test.example.com,DNS:grpc.example.com,DNS:tcp.example.com,DNS:match-test.example.com,DNS:*.wildcard.example.com,DNS:section-test.example.com,DNS:default.example.com,DNS:gateway-tls.test.com,DNS:*.sandbox.example.com,DNS:*.pp2.example.com,DNS:*.sp-allow.example.com,DNS:*.sp-deny.example.com,DNS:*.both-absent.example.com,DNS:both-absent-tls.example.com,DNS:port-only.example.com,DNS:no-hostname-gateway-tls.test.com,DNS:sni421-exact.test.com,DNS:*.sni421.test.com,DNS:*.example.com" \
      2>/dev/null

    cp "$TEMP_DIR/edge-tls.crt" "$CERTS_DIR/server.crt"
    cp "$TEMP_DIR/edge-tls.key" "$CERTS_DIR/server.key"
    cp "$TEMP_DIR/edge-tls.crt" "$CERTS_DIR/ca.pem"
}

# ECDSA companion certificate for the Gateway dual-cert (RSA + ECDSA) listener.
# Shares the gateway-tls.test.com SAN with the RSA edge-tls cert; a unique CN
# (ecdsa-dualcert.example.com) lets the dual-cert test tell which key-type slot
# the server picked during the handshake.
generate_ecdsa_cert() {
    log_info "Generating ECDSA dual-cert companion certificate..."
    openssl ecparam -name prime256v1 -genkey -noout -out "$TEMP_DIR/edge-tls-ecdsa.key" 2>/dev/null
    openssl req -x509 -new -nodes \
      -key "$TEMP_DIR/edge-tls-ecdsa.key" \
      -out "$TEMP_DIR/edge-tls-ecdsa.crt" \
      -days 365 \
      -subj "/CN=ecdsa-dualcert.example.com" \
      -addext "subjectAltName=DNS:gateway-tls.test.com,DNS:ecdsa-dualcert.example.com" \
      2>/dev/null

    cp "$TEMP_DIR/edge-tls-ecdsa.crt" "$CERTS_DIR/server-ecdsa.crt"
    cp "$TEMP_DIR/edge-tls-ecdsa.key" "$CERTS_DIR/server-ecdsa.key"
}

cert_matches_required_sans() {
    local cert_file=$1
    local san_text

    [ -f "$cert_file" ] || return 1

    san_text=$(openssl x509 -in "$cert_file" -noout -text 2>/dev/null | sed -n '/Subject Alternative Name/,+1p') || return 1

    for san in "${REQUIRED_SAN_ENTRIES[@]}"; do
        if [[ "$san_text" != *"$san"* ]]; then
            log_warning "Existing TLS cert is missing SAN entry: $san"
            return 1
        fi
    done

    return 0
}

log_section "Generate TLS certificate"
log_info "temp directory: $TEMP_DIR"
log_info "cert directory: $CERTS_DIR"
log_info "runtime secret output: $SECRET_OUTPUT_FILE"

if [ -f "$CERTS_DIR/server.crt" ] && [ -f "$CERTS_DIR/server.key" ] && cert_matches_required_sans "$CERTS_DIR/server.crt"; then
    log_info "Found existing TLS cert/key with matching SANs, reusing them."
else
    log_info "Generating fresh TLS cert/key for current integration test domains."
    generate_tls_cert
    log_success "TLS cert/key generated."
fi

if [ ! -f "$CERTS_DIR/ca.pem" ] && [ -f "$CERTS_DIR/server.crt" ]; then
    cp "$CERTS_DIR/server.crt" "$CERTS_DIR/ca.pem"
fi

if [ -f "$CERTS_DIR/server-ecdsa.crt" ] && [ -f "$CERTS_DIR/server-ecdsa.key" ]; then
    log_info "Found existing ECDSA cert/key, reusing them."
else
    generate_ecdsa_cert
    log_success "ECDSA cert/key generated."
fi

create_secret_yaml \
    "edge-tls" \
    "edgion-test" \
    "$CERTS_DIR/server.crt" \
    "$CERTS_DIR/server.key" \
    "$SECRET_OUTPUT_FILE"

create_secret_yaml \
    "edge-tls-ecdsa" \
    "edgion-test" \
    "$CERTS_DIR/server-ecdsa.crt" \
    "$CERTS_DIR/server-ecdsa.key" \
    "$SECRET_OUTPUT_DIR/Secret_edgion-test_edge-tls-ecdsa.yaml"

write_status_refgrant_tls_secrets "$CERTS_DIR/server.crt" "$CERTS_DIR/server.key"

log_section "Completed"
log_success "Runtime TLS Secret generated."
log_info "Secret YAML: $SECRET_OUTPUT_FILE"
log_info "Status/RefGrant runtime Secrets: $STATUS_REFGRANT_SECRET_DIR"
log_info "Test cert files: $CERTS_DIR/ca.pem, $CERTS_DIR/server.crt, $CERTS_DIR/server.key"
