#!/bin/bash
# =============================================================================
# mTLS cert generator for integration tests
# - Cert files are written under examples/test/certs/mtls
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

TEMP_DIR="/tmp/edgion-mtls-certs-$$"
mkdir -p "$TEMP_DIR"
trap "rm -rf $TEMP_DIR" EXIT

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CERTS_DIR="$PROJECT_ROOT/examples/test/certs/mtls"
SECRET_OUTPUT_ROOT="${EDGION_GENERATED_SECRET_DIR:-$PROJECT_ROOT/integration_testing/generated_secrets_manual}"
MTLS_SECRET_DIR="$SECRET_OUTPUT_ROOT/EdgionTls/mTLS"
BACKEND_TLS_SECRET_DIR="$SECRET_OUTPUT_ROOT/HTTPRoute/Backend/BackendTLS"
HEADER_CERT_AUTH_SECRET_DIR="$SECRET_OUTPUT_ROOT/EdgionPlugins/HeaderCertAuth"

mkdir -p "$CERTS_DIR"
mkdir -p "$MTLS_SECRET_DIR" "$BACKEND_TLS_SECRET_DIR" "$HEADER_CERT_AUTH_SECRET_DIR"

all_required_certs_exist() {
    local cert_files=(
        "valid-client.crt"
        "valid-client.key"
        "invalid-client.crt"
        "invalid-client.key"
        "nonmatching-client.crt"
        "nonmatching-client.key"
        "chain-client-bundle.crt"
        "chain-client.key"
        "client-ca.crt"
        "intermediate-ca.crt"
        "mtls-server.crt"
        "mtls-server.key"
    )
    local cert_file
    for cert_file in "${cert_files[@]}"; do
        if [ ! -f "$CERTS_DIR/$cert_file" ]; then
            return 1
        fi
    done
    return 0
}

write_mtls_suite_secrets() {
    if [ ! -f "$CERTS_DIR/client-ca.crt" ] || [ ! -f "$CERTS_DIR/intermediate-ca.crt" ]; then
        log_error "Missing client CA files for mTLS Secret generation."
        return 1
    fi
    if [ ! -f "$CERTS_DIR/mtls-server.crt" ] || [ ! -f "$CERTS_DIR/mtls-server.key" ]; then
        log_error "Missing mTLS server cert/key for Secret generation."
        return 1
    fi

    local clientca_b64
    local ca_chain_b64
    local mtls_cert_b64
    local mtls_key_b64

    clientca_b64=$(base64 < "$CERTS_DIR/client-ca.crt" | tr -d '\n')
    ca_chain_b64=$(cat "$CERTS_DIR/client-ca.crt" "$CERTS_DIR/intermediate-ca.crt" | base64 | tr -d '\n')
    mtls_cert_b64=$(base64 < "$CERTS_DIR/mtls-server.crt" | tr -d '\n')
    mtls_key_b64=$(base64 < "$CERTS_DIR/mtls-server.key" | tr -d '\n')

    cat > "$MTLS_SECRET_DIR/Secret_edge_client-ca.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $clientca_b64
EOF

    cat > "$MTLS_SECRET_DIR/Secret_edge_ca-chain.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ca-chain
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $ca_chain_b64
EOF

    cat > "$MTLS_SECRET_DIR/Secret_edge_mtls-server.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: mtls-server
  namespace: edgion-test
type: kubernetes.io/tls
data:
  tls.crt: $mtls_cert_b64
  tls.key: $mtls_key_b64
EOF
}

write_backend_tls_client_secret() {
    if [ ! -f "$CERTS_DIR/valid-client.crt" ] || [ ! -f "$CERTS_DIR/valid-client.key" ]; then
        log_error "Missing valid client cert/key for BackendTLS Secret generation."
        return 1
    fi

    local client_cert_b64
    local client_key_b64
    client_cert_b64=$(base64 < "$CERTS_DIR/valid-client.crt" | tr -d '\n')
    client_key_b64=$(base64 < "$CERTS_DIR/valid-client.key" | tr -d '\n')

    cat > "$BACKEND_TLS_SECRET_DIR/ClientCert_edge_backend-client-cert.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: backend-client-cert
  namespace: edgion-test
type: kubernetes.io/tls
data:
  tls.crt: $client_cert_b64
  tls.key: $client_key_b64
EOF
}

write_header_cert_auth_ca_secret() {
    if [ ! -f "$CERTS_DIR/client-ca.crt" ]; then
        log_error "Missing client-ca.crt for HeaderCertAuth Secret generation."
        return 1
    fi

    local ca_b64
    ca_b64=$(base64 < "$CERTS_DIR/client-ca.crt" | tr -d '\n')

    cat > "$HEADER_CERT_AUTH_SECRET_DIR/01_Secret_default_header-cert-ca.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: header-cert-ca
  namespace: edgion-default
type: Opaque
data:
  ca.crt: $ca_b64
EOF
}

generate_mtls_certs() {
    log_section "Generate mTLS certificate set"

    # Step 1: Client CA
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/client-ca.key" \
        -out "$TEMP_DIR/client-ca.crt" \
        -days 365 \
        -subj "/CN=Edgion Client CA/O=Edgion/OU=Testing" \
        2>/dev/null

    # Step 2: Valid client cert
    openssl req -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/valid-client.key" \
        -out "$TEMP_DIR/valid-client.csr" \
        -subj "/CN=valid-client/O=Edgion/OU=Testing" \
        2>/dev/null

    cat > "$TEMP_DIR/valid-client.ext" <<EOF
subjectAltName=DNS:client1.example.com,DNS:valid-client.edgion.io,DNS:*.edgion.io,email:test@edgion.io
extendedKeyUsage=clientAuth
EOF

    openssl x509 -req \
        -in "$TEMP_DIR/valid-client.csr" \
        -CA "$TEMP_DIR/client-ca.crt" \
        -CAkey "$TEMP_DIR/client-ca.key" \
        -CAcreateserial \
        -out "$TEMP_DIR/valid-client.crt" \
        -days 365 \
        -extfile "$TEMP_DIR/valid-client.ext" \
        2>/dev/null

    # Step 3: Invalid client cert (untrusted CA)
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/untrusted-ca.key" \
        -out "$TEMP_DIR/untrusted-ca.crt" \
        -days 365 \
        -subj "/CN=Untrusted CA/O=Unknown/OU=Unknown" \
        2>/dev/null

    openssl req -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/invalid-client.key" \
        -out "$TEMP_DIR/invalid-client.csr" \
        -subj "/CN=invalid-client/O=Unknown/OU=Unknown" \
        2>/dev/null

    openssl x509 -req \
        -in "$TEMP_DIR/invalid-client.csr" \
        -CA "$TEMP_DIR/untrusted-ca.crt" \
        -CAkey "$TEMP_DIR/untrusted-ca.key" \
        -CAcreateserial \
        -out "$TEMP_DIR/invalid-client.crt" \
        -days 365 \
        2>/dev/null

    # Step 4: Non-matching SAN client cert
    openssl req -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/nonmatching-client.key" \
        -out "$TEMP_DIR/nonmatching-client.csr" \
        -subj "/CN=nonmatching-client/O=Edgion/OU=Testing" \
        2>/dev/null

    cat > "$TEMP_DIR/nonmatching-client.ext" <<EOF
subjectAltName=DNS:other-domain.example.com,email:other@example.com
extendedKeyUsage=clientAuth
EOF

    openssl x509 -req \
        -in "$TEMP_DIR/nonmatching-client.csr" \
        -CA "$TEMP_DIR/client-ca.crt" \
        -CAkey "$TEMP_DIR/client-ca.key" \
        -CAcreateserial \
        -out "$TEMP_DIR/nonmatching-client.crt" \
        -days 365 \
        -extfile "$TEMP_DIR/nonmatching-client.ext" \
        2>/dev/null

    # Step 5: Intermediate CA + chain client cert
    openssl req -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/intermediate-ca.key" \
        -out "$TEMP_DIR/intermediate-ca.csr" \
        -subj "/CN=Edgion Intermediate CA/O=Edgion/OU=Testing" \
        2>/dev/null

    cat > "$TEMP_DIR/intermediate-ca.ext" <<EOF
basicConstraints=CA:TRUE,pathlen:0
keyUsage=keyCertSign,cRLSign
EOF

    openssl x509 -req \
        -in "$TEMP_DIR/intermediate-ca.csr" \
        -CA "$TEMP_DIR/client-ca.crt" \
        -CAkey "$TEMP_DIR/client-ca.key" \
        -CAcreateserial \
        -out "$TEMP_DIR/intermediate-ca.crt" \
        -days 365 \
        -extfile "$TEMP_DIR/intermediate-ca.ext" \
        2>/dev/null

    openssl req -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/chain-client.key" \
        -out "$TEMP_DIR/chain-client.csr" \
        -subj "/CN=chain-client/O=Edgion/OU=Testing" \
        2>/dev/null

    cat > "$TEMP_DIR/chain-client.ext" <<EOF
subjectAltName=DNS:chain-client.edgion.io,DNS:*.edgion.io
extendedKeyUsage=clientAuth
EOF

    openssl x509 -req \
        -in "$TEMP_DIR/chain-client.csr" \
        -CA "$TEMP_DIR/intermediate-ca.crt" \
        -CAkey "$TEMP_DIR/intermediate-ca.key" \
        -CAcreateserial \
        -out "$TEMP_DIR/chain-client.crt" \
        -days 365 \
        -extfile "$TEMP_DIR/chain-client.ext" \
        2>/dev/null

    cat "$TEMP_DIR/chain-client.crt" "$TEMP_DIR/intermediate-ca.crt" > "$TEMP_DIR/chain-client-bundle.crt"

    # Step 6: mTLS server cert
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$TEMP_DIR/mtls-server.key" \
        -out "$TEMP_DIR/mtls-server.crt" \
        -days 365 \
        -subj "/CN=mtls.example.com" \
        -addext "subjectAltName=DNS:mtls.example.com,DNS:mtls-optional.example.com,DNS:mtls-san.example.com,DNS:mtls-chain.example.com" \
        2>/dev/null

    cp "$TEMP_DIR/valid-client.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/valid-client.key" "$CERTS_DIR/"
    cp "$TEMP_DIR/invalid-client.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/invalid-client.key" "$CERTS_DIR/"
    cp "$TEMP_DIR/nonmatching-client.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/nonmatching-client.key" "$CERTS_DIR/"
    cp "$TEMP_DIR/chain-client-bundle.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/chain-client.key" "$CERTS_DIR/"
    cp "$TEMP_DIR/client-ca.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/intermediate-ca.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/mtls-server.crt" "$CERTS_DIR/"
    cp "$TEMP_DIR/mtls-server.key" "$CERTS_DIR/"
}

log_section "Generate mTLS certificate"
log_info "temp directory: $TEMP_DIR"
log_info "cert directory: $CERTS_DIR"
log_info "runtime mTLS secret dir: $MTLS_SECRET_DIR"
log_info "runtime BackendTLS secret dir: $BACKEND_TLS_SECRET_DIR"
log_info "runtime HeaderCertAuth secret dir: $HEADER_CERT_AUTH_SECRET_DIR"

if all_required_certs_exist; then
    log_info "Found existing mTLS cert set, reusing it."
else
    generate_mtls_certs
    log_success "mTLS cert set generated."
fi

write_mtls_suite_secrets
write_backend_tls_client_secret
write_header_cert_auth_ca_secret

log_section "Completed"
log_success "Runtime mTLS-related Secret YAML files generated."
log_info "mTLS suite secrets:"
log_info "  - $MTLS_SECRET_DIR/Secret_edge_client-ca.yaml"
log_info "  - $MTLS_SECRET_DIR/Secret_edge_ca-chain.yaml"
log_info "  - $MTLS_SECRET_DIR/Secret_edge_mtls-server.yaml"
log_info "BackendTLS client cert secret:"
log_info "  - $BACKEND_TLS_SECRET_DIR/ClientCert_edge_backend-client-cert.yaml"
log_info "HeaderCertAuth CA secret:"
log_info "  - $HEADER_CERT_AUTH_SECRET_DIR/01_Secret_default_header-cert-ca.yaml"
