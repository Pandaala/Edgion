#!/bin/bash

# mTLS Certificate Generation Script for Edgion Gateway Testing
# Generates Client CA, client certificates, and mTLS test configurations

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

echo_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

echo_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Create temporary directory using process ID to avoid conflicts
TEMP_DIR="/tmp/edgion-mtls-certs-$$"
mkdir -p "$TEMP_DIR"

# Ensure cleanup on exit
trap "rm -rf $TEMP_DIR; echo_info 'Temporary mTLS certificate files cleaned up'" EXIT

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_DIR="$PROJECT_ROOT/examples/conf"
MTLS_CERTS_DIR="$PROJECT_ROOT/examples/testing/certs/mtls"

# Create mTLS certs directory
mkdir -p "$MTLS_CERTS_DIR"

echo_info "Generating mTLS certificates for Edgion Gateway testing..."
echo_info "Temporary directory: $TEMP_DIR"

# ============================================================
# 1. Generate Client CA (自签名)
# ============================================================
echo_info "Generating Client CA certificate..."
openssl genrsa -out "$TEMP_DIR/ca.key" 2048 2>/dev/null
openssl req -x509 -new -nodes -key "$TEMP_DIR/ca.key" \
  -sha256 -days 365 -out "$TEMP_DIR/ca.crt" \
  -subj "/CN=Edgion Test Client CA" 2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ Client CA certificate generated"
else
    echo_error "Failed to generate Client CA"
    exit 1
fi

# ============================================================
# 2. Generate Valid Client Certificate (signed by CA)
# ============================================================
echo_info "Generating valid client certificate (CN=ValidClient, SAN=client1.example.com)..."
openssl genrsa -out "$TEMP_DIR/valid-client.key" 2048 2>/dev/null
openssl req -new -key "$TEMP_DIR/valid-client.key" \
  -out "$TEMP_DIR/valid-client.csr" \
  -subj "/CN=ValidClient" 2>/dev/null

# Sign with CA
openssl x509 -req -in "$TEMP_DIR/valid-client.csr" \
  -CA "$TEMP_DIR/ca.crt" -CAkey "$TEMP_DIR/ca.key" -CAcreateserial \
  -out "$TEMP_DIR/valid-client.crt" -days 365 -sha256 \
  -extensions v3_req -extfile <(cat <<EOF
[v3_req]
subjectAltName = DNS:client1.example.com
EOF
) 2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ Valid client certificate generated"
else
    echo_error "Failed to generate valid client certificate"
    exit 1
fi

# ============================================================
# 3. Generate Invalid Client Certificate (self-signed, not trusted)
# ============================================================
echo_info "Generating invalid client certificate (self-signed, untrusted)..."
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TEMP_DIR/invalid-client.key" \
  -out "$TEMP_DIR/invalid-client.crt" \
  -days 365 -subj "/CN=UntrustedClient" \
  -addext "subjectAltName=DNS:invalid.example.com" 2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ Invalid client certificate generated"
else
    echo_error "Failed to generate invalid client certificate"
    exit 1
fi

# ============================================================
# 4. Generate Intermediate CA Chain (for verifyDepth=2 testing)
# ============================================================
echo_info "Generating intermediate CA chain..."

# Root CA
openssl genrsa -out "$TEMP_DIR/root-ca.key" 2048 2>/dev/null
openssl req -x509 -new -nodes -key "$TEMP_DIR/root-ca.key" \
  -sha256 -days 365 -out "$TEMP_DIR/root-ca.crt" \
  -subj "/CN=Edgion Test Root CA" 2>/dev/null

# Intermediate CA
openssl genrsa -out "$TEMP_DIR/intermediate-ca.key" 2048 2>/dev/null
openssl req -new -key "$TEMP_DIR/intermediate-ca.key" \
  -out "$TEMP_DIR/intermediate-ca.csr" \
  -subj "/CN=Edgion Test Intermediate CA" 2>/dev/null

# Sign Intermediate CA with Root CA
openssl x509 -req -in "$TEMP_DIR/intermediate-ca.csr" \
  -CA "$TEMP_DIR/root-ca.crt" -CAkey "$TEMP_DIR/root-ca.key" -CAcreateserial \
  -out "$TEMP_DIR/intermediate-ca.crt" -days 365 -sha256 \
  -extensions v3_ca -extfile <(cat <<EOF
[v3_ca]
basicConstraints = CA:TRUE
keyUsage = keyCertSign, cRLSign
EOF
) 2>/dev/null

# Client certificate signed by Intermediate CA
openssl genrsa -out "$TEMP_DIR/chain-client.key" 2048 2>/dev/null
openssl req -new -key "$TEMP_DIR/chain-client.key" \
  -out "$TEMP_DIR/chain-client.csr" \
  -subj "/CN=ChainClient" 2>/dev/null

openssl x509 -req -in "$TEMP_DIR/chain-client.csr" \
  -CA "$TEMP_DIR/intermediate-ca.crt" -CAkey "$TEMP_DIR/intermediate-ca.key" -CAcreateserial \
  -out "$TEMP_DIR/chain-client.crt" -days 365 -sha256 \
  -extensions v3_req -extfile <(cat <<EOF
[v3_req]
subjectAltName = DNS:chain.example.com
EOF
) 2>/dev/null || true

# Create CA bundle (root + intermediate)
cat "$TEMP_DIR/root-ca.crt" "$TEMP_DIR/intermediate-ca.crt" > "$TEMP_DIR/ca-chain.crt" 2>/dev/null || true

if [ -f "$TEMP_DIR/ca-chain.crt" ] && [ -f "$TEMP_DIR/chain-client.crt" ]; then
    echo_info "✓ Intermediate CA chain generated"
else
    echo_error "Failed to generate intermediate CA chain"
    exit 1
fi

# ============================================================
# 5. Generate Client with non-matching SAN (for whitelist testing)
# ============================================================
echo_info "Generating client certificate with non-matching SAN..."
openssl genrsa -out "$TEMP_DIR/nonmatching-client.key" 2048 2>/dev/null
openssl req -new -key "$TEMP_DIR/nonmatching-client.key" \
  -out "$TEMP_DIR/nonmatching-client.csr" \
  -subj "/CN=NonMatchingClient" 2>/dev/null

openssl x509 -req -in "$TEMP_DIR/nonmatching-client.csr" \
  -CA "$TEMP_DIR/ca.crt" -CAkey "$TEMP_DIR/ca.key" -CAcreateserial \
  -out "$TEMP_DIR/nonmatching-client.crt" -days 365 -sha256 \
  -extensions v3_req -extfile <(cat <<EOF
[v3_req]
subjectAltName = DNS:notinwhitelist.example.com
EOF
) 2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ Non-matching SAN client certificate generated"
else
    echo_error "Failed to generate non-matching SAN client certificate"
    exit 1
fi

# ============================================================
# 6. Copy certificates to test directory
# ============================================================
echo_info "Copying certificates to test directory..."
cp "$TEMP_DIR/ca.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/ca.key" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/valid-client.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/valid-client.key" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/invalid-client.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/invalid-client.key" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/root-ca.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/intermediate-ca.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/ca-chain.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/chain-client.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/chain-client.key" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/nonmatching-client.crt" "$MTLS_CERTS_DIR/"
cp "$TEMP_DIR/nonmatching-client.key" "$MTLS_CERTS_DIR/"

echo_info "✓ Certificates copied to: $MTLS_CERTS_DIR/"

# ============================================================
# 7. Create Client CA Secret YAML
# ============================================================
echo_info "Creating Client CA Secret YAML..."
CA_CRT_B64=$(base64 < "$TEMP_DIR/ca.crt" | tr -d '\n')

cat > "$CONF_DIR/Secret_edge_client-ca.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
  namespace: edge
type: Opaque
data:
  ca.crt: $CA_CRT_B64
EOF

echo_info "✓ Client CA Secret created: $CONF_DIR/Secret_edge_client-ca.yaml"

# ============================================================
# 8. Create CA Chain Secret (for verifyDepth=2 testing)
# ============================================================
echo_info "Creating CA Chain Secret YAML..."
CA_CHAIN_B64=$(base64 < "$TEMP_DIR/ca-chain.crt" | tr -d '\n')

cat > "$CONF_DIR/Secret_edge_ca-chain.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ca-chain
  namespace: edge
type: Opaque
data:
  ca.crt: $CA_CHAIN_B64
EOF

echo_info "✓ CA Chain Secret created: $CONF_DIR/Secret_edge_ca-chain.yaml"

echo ""
echo_info "=========================================="
echo_info "mTLS Certificate generation completed!"
echo_info "=========================================="
echo_info "Generated Client CA Secret:"
echo_info "  - $CONF_DIR/Secret_edge_client-ca.yaml"
echo_info "  - $CONF_DIR/Secret_edge_ca-chain.yaml"
echo_info ""
echo_info "Test certificates in: $MTLS_CERTS_DIR/"
echo_info "  - ca.crt / ca.key                  (Client CA)"
echo_info "  - valid-client.crt / .key          (Valid, signed by CA)"
echo_info "  - invalid-client.crt / .key        (Invalid, self-signed)"
echo_info "  - nonmatching-client.crt / .key    (Valid CA, non-matching SAN)"
echo_info "  - chain-client.crt / .key          (Intermediate CA chain)"
echo_info "  - ca-chain.crt                     (Root + Intermediate CA)"
echo ""
echo_warning "Note: These files are gitignored and should not be committed."

