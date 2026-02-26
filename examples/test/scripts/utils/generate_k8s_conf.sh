#!/usr/bin/env bash
#
# generate_k8s_conf.sh - 将本地测试配置转换为 K8s 标准模式
#
# 功能：
#   1. 跳过 EndpointSlice 文件（K8s 会自动创建）
#   2. 为 Service 添加 selector 指向 test-server Pod
#   3. 生成 namespace 和 deployment 配置
#   4. 保留原有目录结构
#
# 使用方法：
#   ./generate_k8s_conf.sh [输出目录]
#
# 示例：
#   ./generate_k8s_conf.sh              # 输出到 examples/k8stest/conf
#   ./generate_k8s_conf.sh /tmp/k8s     # 输出到 /tmp/k8s
#

set -euo pipefail

# 脚本所在目录
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# test 目录
TEST_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
# k8stest 目录
K8S_TEST_DIR="$(cd "$TEST_DIR/.." && pwd)/k8stest"
# conf 源目录
CONF_DIR="$TEST_DIR/conf"
# 默认输出目录
OUTPUT_DIR="${1:-$K8S_TEST_DIR/conf}"
# Deployment 源文件（相对于 workspace root）
WORKSPACE_ROOT="$(cd "$TEST_DIR/../../.." && pwd)"
DEPLOYMENT_SRC="$WORKSPACE_ROOT/edgion-deploy/kubernetes/test/test-server/deployment.yaml"

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
    exit 1
}

# 检查 yq 是否可用
check_yq() {
    if command -v yq &> /dev/null; then
        echo "yq"
    else
        echo "sed"
    fi
}

# 判断文件是否为 EndpointSlice
is_endpoint_slice() {
    local file="$1"
    local filename=$(basename "$file")
    
    # 文件名以 EndpointSlice 开头（如 EndpointSlice_xxx.yaml）
    if [[ "$filename" == EndpointSlice* ]]; then
        return 0
    fi
    
    # 检查顶级 kind 字段（行首匹配，避免误匹配嵌套的 kind）
    if grep -qE "^kind:[[:space:]]*EndpointSlice" "$file" 2>/dev/null; then
        return 0
    fi
    
    return 1
}

# 判断文件是否为 Service
is_service() {
    local file="$1"
    local filename=$(basename "$file")
    
    # 文件名以 Service 开头（如 Service_xxx.yaml）
    if [[ "$filename" == Service* ]]; then
        return 0
    fi
    
    # 检查顶级 kind 字段（行首匹配，避免误匹配 targetRefs 等嵌套的 kind: Service）
    if grep -qE "^kind:[[:space:]]*Service" "$file" 2>/dev/null; then
        return 0
    fi
    
    return 1
}

# 使用 yq 为 Service 添加 selector
add_selector_yq() {
    local input="$1"
    local output="$2"
    
    yq eval '.spec.selector = {"app": "edgion-test-server"}' "$input" > "$output"
}

# 使用 sed 为 Service 添加 selector（fallback）
add_selector_sed() {
    local input="$1"
    local output="$2"
    
    # 检查是否已有 selector
    if grep -q "selector:" "$input"; then
        # 已有 selector，替换它
        sed 's/selector:.*/selector:\n    app: edgion-test-server/' "$input" > "$output"
    else
        # 没有 selector，在 spec: 后添加
        awk '
        /^spec:/ {
            print
            found_spec = 1
            next
        }
        found_spec && /^  [a-z]/ && !added_selector {
            print "  selector:"
            print "    app: edgion-test-server"
            added_selector = 1
        }
        { print }
        END {
            if (found_spec && !added_selector) {
                print "  selector:"
                print "    app: edgion-test-server"
            }
        }
        ' "$input" > "$output"
    fi
}

# 处理 Service 文件
process_service() {
    local input="$1"
    local output="$2"
    local tool="$3"
    
    if [[ "$tool" == "yq" ]]; then
        add_selector_yq "$input" "$output"
    else
        add_selector_sed "$input" "$output"
    fi
}

# 生成 namespace 配置
generate_namespace() {
    local output="$1"
    
    cat > "$output" << 'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: edgion-test
  labels:
    app.kubernetes.io/name: edgion-test
    app.kubernetes.io/component: test
EOF
}

# 处理 Deployment（修改 namespace）
process_deployment() {
    local input="$1"
    local output="$2"
    local tool="$3"
    
    if [[ "$tool" == "yq" ]]; then
        yq eval '.metadata.namespace = "edgion-test"' "$input" > "$output"
    else
        sed 's/namespace: edgion-system/namespace: edgion-test/g' "$input" > "$output"
    fi
}

# 主函数
main() {
    info "开始生成 K8s 测试配置"
    info "源目录: $CONF_DIR"
    info "输出目录: $OUTPUT_DIR"
    
    # 检查源目录
    if [[ ! -d "$CONF_DIR" ]]; then
        error "源配置目录不存在: $CONF_DIR"
    fi
    
    # 检查 deployment 源文件
    if [[ ! -f "$DEPLOYMENT_SRC" ]]; then
        error "Deployment 源文件不存在: $DEPLOYMENT_SRC"
    fi
    
    # 检查工具
    local tool=$(check_yq)
    info "使用 $tool 处理 YAML"
    
    # 清理并创建输出目录
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
    
    # 统计
    local total=0
    local skipped=0
    local services=0
    local copied=0
    
    # 1. 生成 namespace
    generate_namespace "$OUTPUT_DIR/00-namespace.yaml"
    info "生成 00-namespace.yaml"
    
    # 2. 处理 deployment
    process_deployment "$DEPLOYMENT_SRC" "$OUTPUT_DIR/01-deployment.yaml" "$tool"
    info "生成 01-deployment.yaml (namespace: edgion-test)"
    
    # 3. 遍历所有 YAML 文件
    while IFS= read -r -d '' file; do
        ((total++))
        
        # 计算相对路径
        local rel_path="${file#$CONF_DIR/}"
        local output_file="$OUTPUT_DIR/$rel_path"
        local output_dir=$(dirname "$output_file")
        
        # 跳过 EndpointSlice
        if is_endpoint_slice "$file"; then
            ((skipped++))
            warn "跳过 EndpointSlice: $rel_path"
            continue
        fi
        
        # 确保输出目录存在
        mkdir -p "$output_dir"
        
        # 处理 Service
        if is_service "$file"; then
            ((services++))
            process_service "$file" "$output_file" "$tool"
            info "处理 Service: $rel_path (添加 selector)"
        else
            # 直接复制其他文件
            ((copied++))
            cp "$file" "$output_file"
        fi
        
    done < <(find "$CONF_DIR" -name "*.yaml" -type f -print0)
    
    echo ""
    info "========== 完成 =========="
    info "总文件数: $total"
    info "跳过 EndpointSlice: $skipped"
    info "处理 Service: $services"
    info "直接复制: $copied"
    info "输出目录: $OUTPUT_DIR"
    echo ""
    info "使用方法:"
    echo "  kubectl apply -f $OUTPUT_DIR/00-namespace.yaml"
    echo "  kubectl apply -f $OUTPUT_DIR/01-deployment.yaml"
    echo "  kubectl apply -Rf $OUTPUT_DIR/<子目录>"
    echo ""
    info "或者一次性应用所有配置:"
    echo "  kubectl apply -Rf $OUTPUT_DIR"
}

main "$@"
