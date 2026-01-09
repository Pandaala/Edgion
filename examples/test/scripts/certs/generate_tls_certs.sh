#!/bin/bash
# =============================================================================
# TLS Certificate Generation Script for Edgion Gateway Testing
# This script generates self-signed certificates and creates Secret YAML files
# Certificates are generated in /tmp and automatically cleaned up after use
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

# Create temporary directory using process ID to avoid conflicts
TEMP_DIR="/tmp/edgion-certs-$$"
mkdir -p "$TEMP_DIR"

# Ensure cleanup on exit
trap "rm -rf $TEMP_DIR" EXIT

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CONF_DIR="$PROJECT_ROOT/examples/test/conf/base"
CERTS_DIR="$PROJECT_ROOT/examples/test/certs"

# Create directories
mkdir -p "$CONF_DIR"
mkdir -p "$CERTS_DIR"

log_section "生成 TLS 证书"
log_info "临时目录: $TEMP_DIR"
log_info "配置目录: $CONF_DIR"
log_info "证书目录: $CERTS_DIR"

# Check if Secret file already exists
if [ -f "$CONF_DIR/Secret_edgion-test_edge-tls.yaml" ]; then
    log_info "TLS Secret 文件已存在，跳过生成..."
    log_info "  - $CONF_DIR/Secret_edgion-test_edge-tls.yaml"
    echo ""
    log_warning "如需重新生成，请先删除 Secret 文件:"
    log_warning "  rm $CONF_DIR/Secret_edgion-test_edge-tls.yaml"
    exit 0
fi

# Generate single certificate with multiple SANs
log_info "生成多域名证书 (test.example.com, grpc.example.com, tcp.example.com, match-test.example.com)..."
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TEMP_DIR/edge-tls.key" \
  -out "$TEMP_DIR/edge-tls.crt" \
  -days 365 \
  -subj "/CN=test.example.com" \
  -addext "subjectAltName=DNS:test.example.com,DNS:grpc.example.com,DNS:tcp.example.com,DNS:match-test.example.com,DNS:*.wildcard.example.com,DNS:section-test.example.com" \
  2>/dev/null

if [ $? -eq 0 ]; then
    log_success "多域名证书生成成功"
else
    log_error "证书生成失败"
    exit 1
fi

# Function to encode and create Secret YAML
create_secret_yaml() {
    local name=$1
    local namespace=$2
    local cert_file=$3
    local key_file=$4
    local output_file=$5

    log_info "创建 Secret YAML: $(basename $output_file)..."

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
        log_success "Secret YAML 创建成功: $(basename $output_file)"
    else
        log_error "Secret YAML 创建失败: $(basename $output_file)"
        exit 1
    fi
}

# Create Secret YAML for edge-tls
create_secret_yaml \
    "edge-tls" \
    "edgion-test" \
    "$TEMP_DIR/edge-tls.crt" \
    "$TEMP_DIR/edge-tls.key" \
    "$CONF_DIR/Secret_edgion-test_edge-tls.yaml"

# Copy certificate to certs directory for client testing
log_info "复制证书到 certs 目录..."
cp "$TEMP_DIR/edge-tls.crt" "$CERTS_DIR/ca.pem"
cp "$TEMP_DIR/edge-tls.crt" "$CERTS_DIR/server.crt"
cp "$TEMP_DIR/edge-tls.key" "$CERTS_DIR/server.key"

if [ $? -eq 0 ]; then
    log_success "证书已复制到: $CERTS_DIR/"
else
    log_error "证书复制失败"
    exit 1
fi

log_section "完成"
log_success "证书生成完成！"
echo ""
log_info "生成的文件:"
log_info "  Secret YAML:"
log_info "    - $CONF_DIR/Secret_edgion-test_edge-tls.yaml"
log_info ""
log_info "  测试证书:"
log_info "    - $CERTS_DIR/ca.pem"
log_info "    - $CERTS_DIR/server.crt"
log_info "    - $CERTS_DIR/server.key"
log_info ""
log_info "证书包含域名:"
log_info "  - test.example.com (HTTP/HTTPS)"
log_info "  - grpc.example.com (gRPC)"
log_info "  - tcp.example.com (TCP)"
log_info "  - match-test.example.com (Match Tests)"
log_info "  - *.wildcard.example.com (Wildcard)"
log_info "  - section-test.example.com (SectionName)"
