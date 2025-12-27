#!/bin/bash
# Quick test script for LB Policy

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

echo "===================================="
echo "Starting Quick LB Policy Test"
echo "===================================="
echo ""

# 1. 检查 gateway 是否运行
if ! nc -z 127.0.0.1 10080 2>/dev/null; then
    echo "❌ Gateway not running on port 10080"
    echo "Please run: cd examples/testing && ./run_integration_test.sh"
    echo "Or manually start gateway first"
    exit 1
fi

echo "✓ Gateway is running"
echo ""

# 2. 设置 access log 路径
export EDGION_TEST_ACCESS_LOG_PATH="$SCRIPT_DIR/logs/access.log"

if [ ! -f "$EDGION_TEST_ACCESS_LOG_PATH" ]; then
    echo "❌ Access log not found: $EDGION_TEST_ACCESS_LOG_PATH"
    echo "Please start services with run_integration_test.sh first"
    exit 1
fi

echo "✓ Access log found: $EDGION_TEST_ACCESS_LOG_PATH"
echo ""

# 3. 运行测试
echo "Running LB Policy test..."
echo ""
cd "$PROJECT_DIR"
cargo run --example test_client -- -g lb-policy

echo ""
echo "===================================="
echo "Test completed"
echo "===================================="

