#!/bin/bash

# WebSocket 服务器和客户端连接测试脚本

set -e

echo "=========================================="
echo "WebSocket 连接测试"
echo "=========================================="
echo ""

# 检查是否已构建
if [ ! -f "target/debug/examples/test_websocket_server" ] || [ ! -f "target/debug/examples/test_websocket_client" ]; then
    echo "→ 编译测试程序..."
    cargo build --example test_websocket_server --example test_websocket_client
    echo ""
fi

# 启动 WebSocket 服务器（后台运行）
echo "→ 启动 WebSocket 测试服务器..."
cargo run --example test_websocket_server > /tmp/ws_server.log 2>&1 &
SERVER_PID=$!
echo "  服务器 PID: $SERVER_PID"

# 等待服务器启动
echo "→ 等待服务器启动..."
sleep 3

# 检查服务器是否运行
if ! ps -p $SERVER_PID > /dev/null; then
    echo "✗ 服务器启动失败"
    cat /tmp/ws_server.log
    exit 1
fi

echo "✓ 服务器已启动"
echo ""

# 测试函数
test_connection() {
    local protocol=$1
    local url=$2
    local test_name=$3
    
    echo "=========================================="
    echo "测试: $test_name"
    echo "URL: $url"
    echo "=========================================="
    
    if timeout 15 cargo run --example test_websocket_client "$url" 2>&1; then
        echo "✓ $test_name 成功"
        echo ""
        return 0
    else
        echo "✗ $test_name 失败"
        echo ""
        return 1
    fi
}

# 运行测试
SUCCESS=0
FAILED=0

# 测试 1: ws:// 协议
if test_connection "ws" "ws://127.0.0.1:30011/ws" "WS 协议连接"; then
    ((SUCCESS++))
else
    ((FAILED++))
fi

sleep 1

# 测试 2: http:// 协议（应该自动转换为 ws://）
if test_connection "http" "http://127.0.0.1:30012/ws" "HTTP 协议连接（自动转换）"; then
    ((SUCCESS++))
else
    ((FAILED++))
fi

sleep 1

# 测试 3: 连接到不同的服务器
if test_connection "ws" "ws://127.0.0.1:30013/ws" "WS 协议连接（服务器 3）"; then
    ((SUCCESS++))
else
    ((FAILED++))
fi

# 清理：停止服务器
echo "=========================================="
echo "清理..."
echo "=========================================="
kill $SERVER_PID 2>/dev/null || true
sleep 1

# 如果进程还在运行，强制结束
if ps -p $SERVER_PID > /dev/null 2>&1; then
    kill -9 $SERVER_PID 2>/dev/null || true
fi

echo "✓ 服务器已停止"
echo ""

# 输出测试结果
echo "=========================================="
echo "测试结果"
echo "=========================================="
echo "成功: $SUCCESS"
echo "失败: $FAILED"
echo ""

if [ $FAILED -eq 0 ]; then
    echo "✓ 所有测试通过！"
    exit 0
else
    echo "✗ 有测试失败"
    exit 1
fi
