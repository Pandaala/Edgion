#!/bin/bash

# Backend TLS Certificate Generation Script for Edgion Gateway Testing
# This script generates self-signed certificates for backend HTTPS servers
# Certificates are used to test BackendTLSPolicy functionality

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
TEMP_DIR="/tmp/edgion-backend-certs-$$"
mkdir -p "$TEMP_DIR"

# Ensure cleanup on exit
trap "rm -rf $TEMP_DIR; echo_info 'Temporary certificate files cleaned up'" EXIT

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_DIR="$PROJECT_ROOT/examples/test/conf/HTTPRoute/Backend/BackendTLS"
CERTS_DIR="$PROJECT_ROOT/examples/test/certs/backend"

# Create directories
mkdir -p "$CERTS_DIR"

# Check if Secret file already exists
if [ -f "$CONF_DIR/Secret_backend-ca.yaml" ]; then
    echo_info "Backend CA Secret file already exists, skipping generation..."
    echo_info "  - $CONF_DIR/Secret_backend-ca.yaml"
    echo ""
    echo_warning "To regenerate certificates, delete the Secret file and run this script again:"
    echo_warning "  rm $CONF_DIR/Secret_backend-ca.yaml"
    echo_warning "  ./generate_backend_certs.sh"
    exit 0
fi

echo_info "Generating Backend TLS certificates for BackendTLSPolicy testing..."
echo_info "Temporary directory: $TEMP_DIR"

# Step 1: Generate CA certificate (for Gateway to validate backend)
echo_info "Step 1/3: Generating CA certificate..."
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TEMP_DIR/ca.key" \
  -out "$TEMP_DIR/ca.crt" \
  -days 365 \
  -subj "/CN=Backend Test CA/O=Edgion Testing" \
  2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ CA certificate generated successfully"
else
    echo_error "Failed to generate CA certificate"
    exit 1
fi

# Step 2: Generate backend server certificate and key
echo_info "Step 2/3: Generating backend server certificate..."

# Generate private key for backend server
openssl genrsa -out "$TEMP_DIR/server.key" 2048 2>/dev/null

# Generate certificate signing request
openssl req -new \
  -key "$TEMP_DIR/server.key" \
  -out "$TEMP_DIR/server.csr" \
  -subj "/CN=backend.example.com/O=Edgion Backend" \
  2>/dev/null

# Create extensions file for SAN
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

# Sign the certificate with CA
openssl x509 -req \
  -in "$TEMP_DIR/server.csr" \
  -CA "$TEMP_DIR/ca.crt" \
  -CAkey "$TEMP_DIR/ca.key" \
  -CAcreateserial \
  -out "$TEMP_DIR/server.crt" \
  -days 365 \
  -extfile "$TEMP_DIR/server.ext" \
  2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ Backend server certificate generated successfully"
else
    echo_error "Failed to generate backend server certificate"
    exit 1
fi

# Step 3: Create Secret YAML for CA certificate
echo_info "Step 3/3: Creating CA Secret YAML..."

# Read and encode CA certificate (remove newlines for proper YAML)
CA_B64=$(base64 < "$TEMP_DIR/ca.crt" | tr -d '\n')

# Create Secret YAML
cat > "$CONF_DIR/Secret_backend-ca.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: backend-ca
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $CA_B64
EOF

if [ $? -eq 0 ]; then
    echo_info "✓ CA Secret YAML created: Secret_backend-ca.yaml"
else
    echo_error "Failed to create CA Secret YAML"
    exit 1
fi

# Copy certificates to certs directory for backend server
echo_info "Copying certificates to certs directory..."
cp "$TEMP_DIR/server.crt" "$CERTS_DIR/server.crt"
cp "$TEMP_DIR/server.key" "$CERTS_DIR/server.key"
cp "$TEMP_DIR/ca.crt" "$CERTS_DIR/ca.crt"

if [ $? -eq 0 ]; then
    echo_info "✓ Certificates copied to: $CERTS_DIR/"
else
    echo_error "Failed to copy certificates to certs directory"
    exit 1
fi

echo ""
echo_info "=========================================="
echo_info "Backend certificate generation completed!"
echo_info "=========================================="
echo_info "Generated files:"
echo_info "  Secret YAML:"
echo_info "    - $CONF_DIR/Secret_backend-ca.yaml"
echo_info ""
echo_info "  Backend server certificates:"
echo_info "    - $CERTS_DIR/server.crt (Server certificate)"
echo_info "    - $CERTS_DIR/server.key (Server private key)"
echo_info "    - $CERTS_DIR/ca.crt (CA certificate)"
echo_info ""
echo_info "Certificate details:"
echo_info "  - Server CN: backend.example.com"
echo_info "  - SANs: backend.example.com, localhost, 127.0.0.1"
echo_info "  - CA: Backend Test CA"
echo ""
echo_warning "Note: Secret file is gitignored and should not be committed."
echo_info "Temporary certificates in $TEMP_DIR will be automatically cleaned up."
