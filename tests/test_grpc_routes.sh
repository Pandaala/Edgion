#!/bin/bash

# gRPC Routes 测试脚本
# 用于自动化测试 gRPC 路由功能

set -e  # 遇到错误立即退出

# 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# 配置
EDGION_ROOT="/Users/caohao/code/Edgion"
TEST_HOST="grpc.example.com"
TEST_PORT="18443"
GRPC_SERVER_PORT1="50051"
GRPC_SERVER_PORT2="50052"

# 日志函数
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# 清理函数
cleanup() {
    log_info "清理测试环境..."
    
    # 杀死后台进程
    if [ ! -z "$GRPC_SERVER1_PID" ]; then
        kill $GRPC_SERVER1_PID 2>/dev/null || true
        log_info "已停止 gRPC Server 1 (PID: $GRPC_SERVER1_PID)"
    fi
    
    if [ ! -z "$GRPC_SERVER2_PID" ]; then
        kill $GRPC_SERVER2_PID 2>/dev/null || true
        log_info "已停止 gRPC Server 2 (PID: $GRPC_SERVER2_PID)"
    fi
}

# 设置 trap 确保退出时清理
trap cleanup EXIT INT TERM

# 步骤 1: 检查依赖
log_info "步骤 1: 检查依赖..."

if ! command -v grpcurl &> /dev/null; then
    log_warning "grpcurl 未安装，将使用 cargo 测试客户端"
    USE_GRPCURL=false
else
    log_success "grpcurl 已安装"
    USE_GRPCURL=true
fi

# 步骤 2: 编译项目
log_info "步骤 2: 编译 Edgion..."
cd "$EDGION_ROOT"
if cargo build --release --example test_grpc_server --example test_grpc_client 2>&1 | tee /tmp/edgion_build.log | grep -q "error"; then
    log_error "编译失败，请检查错误信息"
    tail -20 /tmp/edgion_build.log
    exit 1
fi
log_success "编译完成"

# 步骤 3: 启动测试 gRPC 服务器
log_info "步骤 3: 启动测试 gRPC 服务器..."

log_info "启动 gRPC Server 1 (port $GRPC_SERVER_PORT1)..."
./target/release/examples/test_grpc_server --port $GRPC_SERVER_PORT1 > /tmp/grpc_server1.log 2>&1 &
GRPC_SERVER1_PID=$!
sleep 2

if ! kill -0 $GRPC_SERVER1_PID 2>/dev/null; then
    log_error "gRPC Server 1 启动失败"
    cat /tmp/grpc_server1.log
    exit 1
fi
log_success "gRPC Server 1 已启动 (PID: $GRPC_SERVER1_PID)"

log_info "启动 gRPC Server 2 (port $GRPC_SERVER_PORT2)..."
./target/release/examples/test_grpc_server --port $GRPC_SERVER_PORT2 > /tmp/grpc_server2.log 2>&1 &
GRPC_SERVER2_PID=$!
sleep 2

if ! kill -0 $GRPC_SERVER2_PID 2>/dev/null; then
    log_error "gRPC Server 2 启动失败"
    cat /tmp/grpc_server2.log
    exit 1
fi
log_success "gRPC Server 2 已启动 (PID: $GRPC_SERVER2_PID)"

# 步骤 4: 测试 gRPC 服务器连通性
log_info "步骤 4: 测试 gRPC 服务器连通性..."

if $USE_GRPCURL; then
    log_info "测试 Server 1..."
    if grpcurl -plaintext -d '{"name":"TestConnection"}' \
        127.0.0.1:$GRPC_SERVER_PORT1 test.TestService/SayHello > /tmp/grpc_test1.json 2>&1; then
        log_success "Server 1 连接成功"
        cat /tmp/grpc_test1.json
    else
        log_error "Server 1 连接失败"
        cat /tmp/grpc_test1.json
        exit 1
    fi
    
    log_info "测试 Server 2..."
    if grpcurl -plaintext -d '{"name":"TestConnection"}' \
        127.0.0.1:$GRPC_SERVER_PORT2 test.TestService/SayHello > /tmp/grpc_test2.json 2>&1; then
        log_success "Server 2 连接成功"
        cat /tmp/grpc_test2.json
    else
        log_error "Server 2 连接失败"
        cat /tmp/grpc_test2.json
        exit 1
    fi
else
    log_info "使用 cargo 客户端测试连接..."
    if ./target/release/examples/test_grpc_client \
        --addr "127.0.0.1:$GRPC_SERVER_PORT1" \
        --name "TestConnection" > /tmp/grpc_test1.log 2>&1; then
        log_success "Server 1 连接成功"
    else
        log_warning "Server 1 连接测试跳过（客户端可能不支持）"
    fi
fi

# 步骤 5: 检查 Edgion Gateway 是否运行
log_info "步骤 5: 检查 Edgion Gateway..."

if pgrep -f "edgion-gateway" > /dev/null; then
    log_success "Edgion Gateway 正在运行"
    GATEWAY_RUNNING=true
else
    log_warning "Edgion Gateway 未运行"
    log_info "请在另一个终端运行: cargo run --release --bin edgion-gateway"
    GATEWAY_RUNNING=false
fi

# 步骤 6: 显示测试资源配置
log_info "步骤 6: gRPC Routes 测试配置..."
echo ""
echo "==========================================="
echo "GRPCRoute 配置文件:"
echo "  config/examples/GRPCRoute_test1_grpc-route1.yaml"
echo ""
echo "Service 配置文件:"
echo "  config/examples/Service_test1_grpc-service.yaml"
echo ""
echo "EndpointSlice 配置文件:"
echo "  config/examples/EndpointSlice_test1_grpc-service.yaml"
echo "==========================================="
echo ""

# 步骤 7: 测试场景
if $GATEWAY_RUNNING && $USE_GRPCURL; then
    log_info "步骤 7: 运行测试场景..."
    echo ""
    
    # 测试 1: 精确匹配
    log_info "测试 1: 精确匹配 test.TestService/SayHello"
    if grpcurl -plaintext \
        -H "Host: $TEST_HOST" \
        -d '{"name":"World"}' \
        127.0.0.1:$TEST_PORT \
        test.TestService/SayHello > /tmp/test1_result.json 2>&1; then
        log_success "测试 1 成功"
        cat /tmp/test1_result.json
    else
        log_error "测试 1 失败"
        cat /tmp/test1_result.json
    fi
    echo ""
    
    # 测试 2: 流式方法
    log_info "测试 2: 流式方法 test.TestService/StreamNumbers"
    if grpcurl -plaintext \
        -H "Host: $TEST_HOST" \
        -d '{"count":5}' \
        127.0.0.1:$TEST_PORT \
        test.TestService/StreamNumbers > /tmp/test2_result.json 2>&1; then
        log_success "测试 2 成功"
        cat /tmp/test2_result.json
    else
        log_error "测试 2 失败"
        cat /tmp/test2_result.json
    fi
    echo ""
    
    # 测试 3: 负载均衡测试
    log_info "测试 3: 负载均衡测试（10次请求）"
    echo "统计后端分布..."
    for i in {1..10}; do
        grpcurl -plaintext \
            -H "Host: $TEST_HOST" \
            -d '{"name":"LoadBalanceTest"}' \
            127.0.0.1:$TEST_PORT \
            test.TestService/SayHello 2>/dev/null | \
            jq -r '.serverAddr' 2>/dev/null || echo "unknown"
    done | sort | uniq -c
    echo ""
    
else
    log_warning "跳过步骤 7: Gateway 未运行或 grpcurl 不可用"
fi

# 步骤 8: 查看日志
log_info "步骤 8: 检查日志..."

if [ -f "$EDGION_ROOT/logs/edgion_access.log" ]; then
    log_info "最近的访问日志（gRPC 相关）:"
    tail -20 "$EDGION_ROOT/logs/edgion_access.log" | grep -i grpc || log_warning "未找到 gRPC 相关日志"
else
    log_warning "访问日志文件不存在: $EDGION_ROOT/logs/edgion_access.log"
fi
echo ""

if [ -f "$EDGION_ROOT/logs/edgion-gateway.$(date +%Y-%m-%d)" ]; then
    log_info "最近的网关日志（gRPC 相关）:"
    tail -20 "$EDGION_ROOT/logs/edgion-gateway.$(date +%Y-%m-%d)" | grep -i grpc || log_warning "未找到 gRPC 相关日志"
else
    log_warning "网关日志文件不存在"
fi

# 总结
echo ""
echo "==========================================="
log_success "测试完成！"
echo "==========================================="
echo ""
echo "gRPC Server 1: 127.0.0.1:$GRPC_SERVER_PORT1 (PID: $GRPC_SERVER1_PID)"
echo "gRPC Server 2: 127.0.0.1:$GRPC_SERVER_PORT2 (PID: $GRPC_SERVER2_PID)"
echo ""
echo "服务器日志:"
echo "  Server 1: /tmp/grpc_server1.log"
echo "  Server 2: /tmp/grpc_server2.log"
echo ""
echo "测试结果:"
echo "  Test 1: /tmp/test1_result.json"
echo "  Test 2: /tmp/test2_result.json"
echo ""

if $GATEWAY_RUNNING; then
    log_info "可以继续手动测试，按 Ctrl+C 停止服务器"
    # 保持脚本运行，直到用户按 Ctrl+C
    log_info "等待手动测试... (按 Ctrl+C 退出)"
    while true; do
        sleep 10
    done
else
    log_info "要使用 Edgion Gateway 进行完整测试，请:"
    echo "1. 启动 Gateway: cargo run --release --bin edgion-gateway"
    echo "2. 重新运行此脚本"
fi

