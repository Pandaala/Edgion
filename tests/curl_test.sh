#!/bin/bash
# Edgion Gateway curl 测试脚本
# 用于测试 HTTPRoute 和 EdgionPlugins 配置

# 配置
GATEWAY_HOST="127.0.0.1"
GATEWAY_PORT="18080"
BASE_URL="https://${GATEWAY_HOST}:${GATEWAY_PORT}"

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo_title() {
    echo -e "\n${BLUE}========================================${NC}"
    echo -e "${BLUE}$1${NC}"
    echo -e "${BLUE}========================================${NC}"
}

echo_test() {
    echo -e "\n${YELLOW}▶ $1${NC}"
}

# ============================================================================
# 规则1: /aaa/* - 规则级别 ExtensionRef (EdgionPlugins)
# Hostname: aaa.example.com
# ============================================================================
echo_title "规则1: 测试规则级别 ExtensionRef (EdgionPlugins)"

echo_test "测试 /aaa/doc/ 路径 (PathPrefix 匹配)"
curl -k -v \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/aaa/doc/test.html" \
    2>&1 | grep -E "(< HTTP|< X-|> Host)"

echo_test "测试 /aaa/exact 路径 (PathPrefix 匹配)"
curl -k -v \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/aaa/exact/path" \
    2>&1 | grep -E "(< HTTP|< X-|> Host)"

echo_test "测试 /aaa/doc/ 查看响应头 (应包含 EdgionPlugins 添加的头)"
curl -k -I \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/aaa/doc/"

# ============================================================================
# 规则2: /bbb/* - 后端级别 ExtensionRef (EdgionPlugins)
# Hostname: bbb.example.com
# ============================================================================
echo_title "规则2: 测试后端级别 ExtensionRef (EdgionPlugins)"

echo_test "测试 /bbb/123 路径 (PathPrefix 匹配)"
curl -k -v \
    -H "Host: bbb.example.com" \
    "${BASE_URL}/bbb/123/test" \
    2>&1 | grep -E "(< HTTP|< X-|> Host)"

echo_test "测试 /bbb/{id1}/ccc/{id2}/eee 路径参数 (Exact 匹配)"
curl -k -v \
    -H "Host: bbb.example.com" \
    "${BASE_URL}/bbb/user001/ccc/order999/eee" \
    2>&1 | grep -E "(< HTTP|< X-|> Host)"

echo_test "测试负载均衡 - 多次请求观察 weight 分配"
for i in {1..5}; do
    echo "Request $i:"
    curl -k -s -o /dev/null -w "HTTP %{http_code}\n" \
        -H "Host: bbb.example.com" \
        "${BASE_URL}/bbb/123/test"
done

# ============================================================================
# 规则3: /ccc/* - 规则级别 filter + ExtensionRef
# Hostname: aaa.example.com
# ============================================================================
echo_title "规则3: 测试规则级别 filter + ExtensionRef 混合使用"

echo_test "测试 /ccc/api/ 路径 (应同时应用直接filter和EdgionPlugins)"
curl -k -v \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/ccc/api/v1/users" \
    2>&1 | grep -E "(< HTTP|< X-|> Host)"

echo_test "测试 /ccc/{id1}/{id2}/ddd 路径参数 (Exact 匹配)"
curl -k -v \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/ccc/abc/xyz/ddd" \
    2>&1 | grep -E "(< HTTP|< X-|> Host)"

echo_test "测试 /ccc/api/ 查看所有响应头"
curl -k -I \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/ccc/api/"

# ============================================================================
# 错误路径测试
# ============================================================================
echo_title "错误路径测试"

echo_test "测试不存在的路径 (应返回 404)"
curl -k -s -o /dev/null -w "HTTP %{http_code}\n" \
    -H "Host: aaa.example.com" \
    "${BASE_URL}/not-exist/path"

echo_test "测试错误的 Host (应返回 404 或默认响应)"
curl -k -s -o /dev/null -w "HTTP %{http_code}\n" \
    -H "Host: unknown.example.com" \
    "${BASE_URL}/aaa/doc/"

# ============================================================================
# 性能测试 (简单)
# ============================================================================
echo_title "简单性能测试"

echo_test "连续10次请求测试响应时间"
for i in {1..10}; do
    time_total=$(curl -k -s -o /dev/null -w "%{time_total}" \
        -H "Host: aaa.example.com" \
        "${BASE_URL}/aaa/doc/")
    echo "Request $i: ${time_total}s"
done

echo -e "\n${GREEN}✓ 测试完成${NC}\n"

