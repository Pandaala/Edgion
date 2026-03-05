#!/bin/bash

# Backend TLS cert generator for integration tests
# - Cert files are written under examples/test/certs/backend
# - Secret YAML is written to runtime directory only (never conf/)

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

echo_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

echo_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

TEMP_DIR="/tmp/edgion-backend-certs-$$"
mkdir -p "$TEMP_DIR"
trap "rm -rf $TEMP_DIR" EXIT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CERTS_DIR="$PROJECT_ROOT/examples/test/certs/backend"
SECRET_OUTPUT_ROOT="${EDGION_GENERATED_SECRET_DIR:-$PROJECT_ROOT/integration_testing/generated_secrets_manual}"
SECRET_OUTPUT_DIR="$SECRET_OUTPUT_ROOT/HTTPRoute/Backend/BackendTLS"
SECRET_OUTPUT_FILE="$SECRET_OUTPUT_DIR/Secret_backend-ca.yaml"

mkdir -p "$CERTS_DIR"
mkdir -p "$SECRET_OUTPUT_DIR"

create_backend_ca_secret() {
    local ca_b64
    ca_b64=$(base64 < "$CERTS_DIR/ca.crt" | tr -d '\n')
    cat > "$SECRET_OUTPUT_FILE" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: backend-ca
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $ca_b64
EOF
}

generate_backend_certs() {
    echo_info "Generating Backend TLS certificates..."

    openssl req -x509 -newkey rsa:2048 -nodes \
      -keyout "$TEMP_DIR/ca.key" \
      -out "$TEMP_DIR/ca.crt" \
      -days 365 \
      -subj "/CN=Backend Test CA/O=Edgion Testing" \
      2>/dev/null

    openssl genrsa -out "$TEMP_DIR/server.key" 2048 2>/dev/null

    openssl req -new \
      -key "$TEMP_DIR/server.key" \
      -out "$TEMP_DIR/server.csr" \
      -subj "/CN=backend.example.com/O=Edgion Backend" \
      2>/dev/null

    cat > "$TEMP_DIR/server.ext" <<EOF
authorityKeyIdentifier=keyid,issuer
basicConstraints=CA:FALSE
keyUsage = digitalSignature, keyEncipherment
subjectAltName = @alt_names

[alt_names]
DNS.1 = backend.example.com
DNS.2 = localhost
IP.1 = 127.0.0.1
EOF

    openssl x509 -req \
      -in "$TEMP_DIR/server.csr" \
      -CA "$TEMP_DIR/ca.crt" \
      -CAkey "$TEMP_DIR/ca.key" \
      -CAcreateserial \
      -out "$TEMP_DIR/server.crt" \
      -days 365 \
      -extfile "$TEMP_DIR/server.ext" \
      2>/dev/null

    cp "$TEMP_DIR/server.crt" "$CERTS_DIR/server.crt"
    cp "$TEMP_DIR/server.key" "$CERTS_DIR/server.key"
    cp "$TEMP_DIR/ca.crt" "$CERTS_DIR/ca.crt"
}

echo_info "Backend cert dir: $CERTS_DIR"
echo_info "Runtime Secret output: $SECRET_OUTPUT_FILE"

if [ -f "$CERTS_DIR/server.crt" ] && [ -f "$CERTS_DIR/server.key" ] && [ -f "$CERTS_DIR/ca.crt" ]; then
    echo_info "Found existing backend cert files, reusing them."
else
    generate_backend_certs
fi

if [ ! -f "$CERTS_DIR/ca.crt" ]; then
    echo_error "Missing $CERTS_DIR/ca.crt"
    exit 1
fi

create_backend_ca_secret

echo ""
echo_info "=========================================="
echo_info "Backend certificate generation completed"
echo_info "=========================================="
echo_info "Runtime Secret YAML:"
echo_info "  - $SECRET_OUTPUT_FILE"
echo_info "Backend cert files:"
echo_info "  - $CERTS_DIR/server.crt"
echo_info "  - $CERTS_DIR/server.key"
echo_info "  - $CERTS_DIR/ca.crt"
echo_warning "Note: Secret YAML is runtime-generated and should not be committed."
