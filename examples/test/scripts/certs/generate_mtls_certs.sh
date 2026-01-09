#!/bin/bash
# =============================================================================
# mTLS Certificate Generation Script for Edgion Gateway Testing
# 生成双向 TLS 测试所需的证书和 Secret YAML 文件
# =============================================================================

set -e

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $1"
}

log_error() {
    echo -e "${RED}[✗]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[!]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

# Create temporary directory
TEMP_DIR="/tmp/edgion-mtls-certs-$$"
mkdir -p "$TEMP_DIR"

# Ensure cleanup on exit
trap "rm -rf $TEMP_DIR" EXIT

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_DIR="$PROJECT_ROOT/examples/test/conf/EdgionTls/mTLS"
CERTS_DIR="$PROJECT_ROOT/examples/test/certs/mtls"

# Create directories
mkdir -p "$CONF_DIR"
mkdir -p "$CERTS_DIR"

log_section "生成 mTLS 证书"
log_info "临时目录: $TEMP_DIR"
log_info "配置目录: $CONF_DIR"
log_info "证书目录: $CERTS_DIR"

# Check if certificates already exist
if [ -f "$CERTS_DIR/valid-client.crt" ]; then
    log_info "mTLS 证书已存在，跳过生成..."
    log_warning "如需重新生成，请先删除证书目录:"
    log_warning "  rm -rf $CERTS_DIR"
    exit 0
fi

# =============================================================================
# Step 1: Generate Client CA (用于签发客户端证书)
# =============================================================================
log_section "生成客户端 CA 证书"

openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/client-ca.key" \
    -out "$TEMP_DIR/client-ca.crt" \
    -days 365 \
    -subj "/CN=Edgion Client CA/O=Edgion/OU=Testing" \
    2>/dev/null

log_success "客户端 CA 生成成功"

# =============================================================================
# Step 2: Generate Valid Client Certificate (有效的客户端证书)
# =============================================================================
log_section "生成有效客户端证书"

# Create CSR
openssl req -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/valid-client.key" \
    -out "$TEMP_DIR/valid-client.csr" \
    -subj "/CN=valid-client/O=Edgion/OU=Testing" \
    2>/dev/null

# Create extension file for SAN
# Note: client1.example.com is used to match EdgionTls allowedSans whitelist
cat > "$TEMP_DIR/valid-client.ext" << EOF
subjectAltName=DNS:client1.example.com,DNS:valid-client.edgion.io,DNS:*.edgion.io,email:test@edgion.io
extendedKeyUsage=clientAuth
EOF

# Sign with Client CA
openssl x509 -req \
    -in "$TEMP_DIR/valid-client.csr" \
    -CA "$TEMP_DIR/client-ca.crt" \
    -CAkey "$TEMP_DIR/client-ca.key" \
    -CAcreateserial \
    -out "$TEMP_DIR/valid-client.crt" \
    -days 365 \
    -extfile "$TEMP_DIR/valid-client.ext" \
    2>/dev/null

log_success "有效客户端证书生成成功"

# =============================================================================
# Step 3: Generate Invalid Client Certificate (不受信任的 CA 签发)
# =============================================================================
log_section "生成无效客户端证书（不受信任的 CA）"

# Create a separate untrusted CA
openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/untrusted-ca.key" \
    -out "$TEMP_DIR/untrusted-ca.crt" \
    -days 365 \
    -subj "/CN=Untrusted CA/O=Unknown/OU=Unknown" \
    2>/dev/null

# Create CSR
openssl req -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/invalid-client.key" \
    -out "$TEMP_DIR/invalid-client.csr" \
    -subj "/CN=invalid-client/O=Unknown/OU=Unknown" \
    2>/dev/null

# Sign with untrusted CA
openssl x509 -req \
    -in "$TEMP_DIR/invalid-client.csr" \
    -CA "$TEMP_DIR/untrusted-ca.crt" \
    -CAkey "$TEMP_DIR/untrusted-ca.key" \
    -CAcreateserial \
    -out "$TEMP_DIR/invalid-client.crt" \
    -days 365 \
    2>/dev/null

log_success "无效客户端证书生成成功"

# =============================================================================
# Step 4: Generate Non-matching SAN Client Certificate
# =============================================================================
log_section "生成 SAN 不匹配的客户端证书"

# Create CSR
openssl req -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/nonmatching-client.key" \
    -out "$TEMP_DIR/nonmatching-client.csr" \
    -subj "/CN=nonmatching-client/O=Edgion/OU=Testing" \
    2>/dev/null

# Create extension file with non-matching SAN
cat > "$TEMP_DIR/nonmatching-client.ext" << EOF
subjectAltName=DNS:other-domain.example.com,email:other@example.com
extendedKeyUsage=clientAuth
EOF

# Sign with Client CA
openssl x509 -req \
    -in "$TEMP_DIR/nonmatching-client.csr" \
    -CA "$TEMP_DIR/client-ca.crt" \
    -CAkey "$TEMP_DIR/client-ca.key" \
    -CAcreateserial \
    -out "$TEMP_DIR/nonmatching-client.crt" \
    -days 365 \
    -extfile "$TEMP_DIR/nonmatching-client.ext" \
    2>/dev/null

log_success "SAN 不匹配的客户端证书生成成功"

# =============================================================================
# Step 5: Generate Intermediate CA and Chain Client Certificate
# =============================================================================
log_section "生成中间 CA 和证书链客户端证书"

# Create Intermediate CA
openssl req -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/intermediate-ca.key" \
    -out "$TEMP_DIR/intermediate-ca.csr" \
    -subj "/CN=Edgion Intermediate CA/O=Edgion/OU=Testing" \
    2>/dev/null

cat > "$TEMP_DIR/intermediate-ca.ext" << EOF
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

# Create Chain Client Certificate
openssl req -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/chain-client.key" \
    -out "$TEMP_DIR/chain-client.csr" \
    -subj "/CN=chain-client/O=Edgion/OU=Testing" \
    2>/dev/null

cat > "$TEMP_DIR/chain-client.ext" << EOF
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

# Create chain bundle (client cert + intermediate CA)
cat "$TEMP_DIR/chain-client.crt" "$TEMP_DIR/intermediate-ca.crt" > "$TEMP_DIR/chain-client-bundle.crt"

log_success "证书链客户端证书生成成功"

# =============================================================================
# Step 6: Generate mTLS Server Certificate (for Gateway)
# =============================================================================
log_section "生成 mTLS 服务端证书"

openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$TEMP_DIR/mtls-server.key" \
    -out "$TEMP_DIR/mtls-server.crt" \
    -days 365 \
    -subj "/CN=mtls.example.com" \
    -addext "subjectAltName=DNS:mtls.example.com,DNS:mtls-optional.example.com,DNS:mtls-san.example.com,DNS:mtls-chain.example.com" \
    2>/dev/null

log_success "mTLS 服务端证书生成成功"

# =============================================================================
# Step 7: Copy certificates to certs directory
# =============================================================================
log_section "复制证书到目标目录"

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

log_success "证书复制完成"

# =============================================================================
# Step 8: Create Secret YAML files
# =============================================================================
log_section "生成 Secret YAML 文件"

# Client CA Secret
CLIENTCA_B64=$(base64 < "$TEMP_DIR/client-ca.crt" | tr -d '\n')
cat > "$CONF_DIR/Secret_edge_client-ca.yaml" << EOF
apiVersion: v1
kind: Secret
metadata:
  name: client-ca
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $CLIENTCA_B64
EOF
log_success "创建 Secret_edge_client-ca.yaml"

# CA Chain Secret (includes intermediate CA)
cat "$TEMP_DIR/client-ca.crt" "$TEMP_DIR/intermediate-ca.crt" > "$TEMP_DIR/ca-chain.crt"
CACHAIN_B64=$(base64 < "$TEMP_DIR/ca-chain.crt" | tr -d '\n')
cat > "$CONF_DIR/Secret_edge_ca-chain.yaml" << EOF
apiVersion: v1
kind: Secret
metadata:
  name: ca-chain
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $CACHAIN_B64
EOF
log_success "创建 Secret_edge_ca-chain.yaml"

# mTLS Server TLS Secret
MTLSCERT_B64=$(base64 < "$TEMP_DIR/mtls-server.crt" | tr -d '\n')
MTLSKEY_B64=$(base64 < "$TEMP_DIR/mtls-server.key" | tr -d '\n')
cat > "$CONF_DIR/Secret_edge_mtls-server.yaml" << EOF
apiVersion: v1
kind: Secret
metadata:
  name: mtls-server
  namespace: edgion-test
type: kubernetes.io/tls
data:
  tls.crt: $MTLSCERT_B64
  tls.key: $MTLSKEY_B64
EOF
log_success "创建 Secret_edge_mtls-server.yaml"

# =============================================================================
# Summary
# =============================================================================
log_section "完成"
log_success "mTLS 证书生成完成！"
echo ""
log_info "生成的证书文件 ($CERTS_DIR/):"
log_info "  - valid-client.crt/key      有效客户端证书"
log_info "  - invalid-client.crt/key    无效客户端证书（不受信任的 CA）"
log_info "  - nonmatching-client.crt/key SAN 不匹配的客户端证书"
log_info "  - chain-client-bundle.crt   带证书链的客户端证书"
log_info "  - chain-client.key          证书链客户端私钥"
log_info "  - client-ca.crt             客户端 CA 证书"
log_info "  - intermediate-ca.crt       中间 CA 证书"
log_info "  - mtls-server.crt/key       mTLS 服务端证书"
echo ""
log_info "生成的 Secret YAML 文件 ($CONF_DIR/):"
log_info "  - Secret_edge_client-ca.yaml    客户端 CA Secret"
log_info "  - Secret_edge_ca-chain.yaml     CA 证书链 Secret"
log_info "  - Secret_edge_mtls-server.yaml  mTLS 服务端证书 Secret"
