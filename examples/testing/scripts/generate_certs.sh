#!/bin/bash

# TLS Certificate Generation Script for Edgion Gateway Testing
# This script generates self-signed certificates and creates Kubernetes Secret YAML files
# Certificates are generated in /tmp and automatically cleaned up after use

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
TEMP_DIR="/tmp/edgion-certs-$$"
mkdir -p "$TEMP_DIR"

# Ensure cleanup on exit
trap "rm -rf $TEMP_DIR; echo_info 'Temporary certificate files cleaned up'" EXIT

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
CONF_DIR="$PROJECT_ROOT/examples/conf"

# Check if Secret file already exists
if [ -f "$CONF_DIR/Secret_edge_tls.yaml" ]; then
    echo_info "TLS Secret file already exists, skipping generation..."
    echo_info "  - $CONF_DIR/Secret_edge_tls.yaml"
    echo ""
    echo_warning "To regenerate certificates, delete the Secret file and run this script again:"
    echo_warning "  rm $CONF_DIR/Secret_edge_tls.yaml"
    echo_warning "  ./scripts/generate_certs.sh"
    exit 0
fi

echo_info "Generating TLS certificates for Edgion Gateway testing..."
echo_info "Temporary directory: $TEMP_DIR"

# Generate single certificate with multiple SANs (test.example.com + grpc.example.com)
echo_info "Generating certificate with multiple domains (test.example.com, grpc.example.com)..."
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TEMP_DIR/edge-tls.key" \
  -out "$TEMP_DIR/edge-tls.crt" \
  -days 365 \
  -subj "/CN=test.example.com" \
  -addext "subjectAltName=DNS:test.example.com,DNS:grpc.example.com" \
  2>/dev/null

if [ $? -eq 0 ]; then
    echo_info "✓ Certificate with multiple domains generated successfully"
else
    echo_error "Failed to generate certificate"
    exit 1
fi

# Function to encode and create Secret YAML
create_secret_yaml() {
    local name=$1
    local namespace=$2
    local cert_file=$3
    local key_file=$4
    local output_file=$5

    echo_info "Creating Secret YAML: $(basename $output_file)..."

    # Read and encode certificate and key (remove newlines for proper YAML)
    CERT_B64=$(base64 < "$cert_file" | tr -d '\n')
    KEY_B64=$(base64 < "$key_file" | tr -d '\n')

    # Create Secret YAML
    cat > "$output_file" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: $name
  namespace: $namespace
type: kubernetes.io/tls
data:
  tls.crt: $CERT_B64
  tls.key: $KEY_B64
EOF

    if [ $? -eq 0 ]; then
        echo_info "✓ Secret YAML created: $(basename $output_file)"
    else
        echo_error "Failed to create Secret YAML: $(basename $output_file)"
        exit 1
    fi
}

# Create Secret YAML for edge-tls
create_secret_yaml \
    "edge-tls" \
    "edge" \
    "$TEMP_DIR/edge-tls.crt" \
    "$TEMP_DIR/edge-tls.key" \
    "$CONF_DIR/Secret_edge_tls.yaml"

echo ""
echo_info "=========================================="
echo_info "Certificate generation completed!"
echo_info "=========================================="
echo_info "Generated Secret YAML file:"
echo_info "  - $CONF_DIR/Secret_edge_tls.yaml"
echo_info ""
echo_info "Certificate includes domains:"
echo_info "  - test.example.com (HTTPS)"
echo_info "  - grpc.example.com (gRPC-HTTPS)"
echo ""
echo_warning "Note: Secret file is gitignored and should not be committed."
echo_info "Temporary certificates in $TEMP_DIR will be automatically cleaned up."

