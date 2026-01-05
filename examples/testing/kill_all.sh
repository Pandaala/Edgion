#!/bin/bash
# 停止所有 Edgion 相关的后台进程

echo "🛑 停止所有 Edgion 相关进程..."
echo ""

# 停止 edgion-controller
PIDS=$(pgrep -f edgion-controller)
if [ ! -z "$PIDS" ]; then
    echo "停止 edgion-controller:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "edgion-controller: 未运行"
fi

# 停止 edgion-gateway
PIDS=$(pgrep -f edgion-gateway)
if [ ! -z "$PIDS" ]; then
    echo "停止 edgion-gateway:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "edgion-gateway: 未运行"
fi

# 停止 gRPC 测试服务器
PIDS=$(pgrep -f test_grpc_server)
if [ ! -z "$PIDS" ]; then
    echo "停止 gRPC 测试服务器:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "gRPC 测试服务器: 未运行"
fi

# 停止 WebSocket 测试服务器
PIDS=$(pgrep -f test_websocket_server)
if [ ! -z "$PIDS" ]; then
    echo "停止 WebSocket 测试服务器:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "WebSocket 测试服务器: 未运行"
fi

# 停止 HTTP 测试服务器
PIDS=$(pgrep -f test_http_server)
if [ ! -z "$PIDS" ]; then
    echo "停止 HTTP 测试服务器:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "HTTP 测试服务器: 未运行"
fi

# 停止 gRPC 测试客户端
PIDS=$(pgrep -f test_grpc_client)
if [ ! -z "$PIDS" ]; then
    echo "停止 gRPC 测试客户端:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "gRPC 测试客户端: 未运行"
fi

# 停止 WebSocket 测试客户端
PIDS=$(pgrep -f test_websocket_client)
if [ ! -z "$PIDS" ]; then
    echo "停止 WebSocket 测试客户端:"
    for PID in $PIDS; do
        kill -9 $PID 2>/dev/null && echo "  ✓ PID: $PID"
    done
else
    echo "WebSocket 测试客户端: 未运行"
fi

echo ""
echo "✅ 完成！"

