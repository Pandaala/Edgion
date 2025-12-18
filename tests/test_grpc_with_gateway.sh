#!/bin/bash
# gRPC Gateway 完整测试脚本

echo "======================================"
echo "gRPC Gateway 完整测试"
echo "======================================"
echo ""
echo "⚠️  注意: 此脚本需要在 3 个终端中运行："
echo ""
echo "Terminal 1: 运行此脚本启动所有服务"
echo "Terminal 2-4: 分别启动 gRPC server, Controller, Gateway"
echo ""
echo "======================================"
echo ""

echo "📋 测试前检查清单:"
echo ""
echo "1. gRPC 测试服务器需要运行在 30021-30024 端口"
echo "   启动命令: cargo run --example test_grpc_server"
echo ""
echo "2. Controller 需要运行（提供配置给 Gateway）"
echo "   启动命令: cargo run --bin edgion-controller -- -c ./config/edgion-controller.toml"
echo ""
echo "3. Gateway 需要运行并加载 GRPCRoute"
echo "   启动命令: cargo run --bin edgion-gateway -- -c ./config/edgion-gateway.toml"
echo ""
echo "4. 配置文件:"
echo "   - config/examples/GRPCRoute_test1_grpc-route1.yaml (已配置 http18080 和 https18443)"
echo "   - config/examples/Service_test1_grpc-service.yaml"
echo "   - config/examples/EndpointSlice_test1_grpc-service.yaml"
echo ""
echo "======================================"
echo ""

# 检查进程
echo "🔍 检查当前运行的服务:"
echo ""

if pgrep -q -f "test_grpc_server"; then
    echo "✅ gRPC Server 正在运行 (PID: $(pgrep -f test_grpc_server))"
else
    echo "❌ gRPC Server 未运行"
    echo "   请在另一个终端运行: cargo run --example test_grpc_server"
fi

if pgrep -q -f "edgion-controller"; then
    echo "✅ Controller 正在运行 (PID: $(pgrep -f edgion-controller))"
else
    echo "❌ Controller 未运行"
    echo "   请在另一个终端运行: cargo run --bin edgion-controller -- -c ./config/edgion-controller.toml"
fi

if pgrep -q -f "edgion-gateway"; then
    echo "✅ Gateway 正在运行 (PID: $(pgrep -f edgion-gateway))"
else
    echo "❌ Gateway 未运行"
    echo "   请在另一个终端运行: cargo run --bin edgion-gateway -- -c ./config/edgion-gateway.toml"
fi

echo ""
echo "======================================"
echo ""

# 检查所有服务是否都在运行
if ! pgrep -q -f "test_grpc_server" || ! pgrep -q -f "edgion-controller" || ! pgrep -q -f "edgion-gateway"; then
    echo "⚠️  请先启动所有必需的服务，然后重新运行此脚本进行测试"
    exit 1
fi

echo "✅ 所有服务都在运行，开始测试..."
echo ""

# 等待服务完全启动
echo "⏳ 等待 5 秒让服务完全启动..."
sleep 5

echo ""
echo "======================================"
echo "测试 1: 直连 gRPC Server (验证服务器工作正常)"
echo "======================================"
echo ""
echo "命令: grpcurl -plaintext -proto examples/proto/test_service.proto -d '{\"name\":\"DirectTest\"}' 127.0.0.1:30021 test.TestService/SayHello"
echo ""
grpcurl -plaintext \
  -proto examples/proto/test_service.proto \
  -d '{"name":"DirectTest"}' \
  127.0.0.1:30021 \
  test.TestService/SayHello

if [ $? -eq 0 ]; then
    echo ""
    echo "✅ 测试 1 通过: gRPC Server 工作正常"
else
    echo ""
    echo "❌ 测试 1 失败: gRPC Server 连接失败"
    exit 1
fi

echo ""
echo "======================================"
echo "测试 2: 通过 Gateway (HTTP/2 h2c - 端口 18080)"
echo "======================================"
echo ""
echo "命令: grpcurl -plaintext -proto examples/proto/test_service.proto -H \"Host: grpc.example.com\" -d '{\"name\":\"GatewayTest\"}' 127.0.0.1:18080 test.TestService/SayHello"
echo ""
grpcurl -plaintext \
  -proto examples/proto/test_service.proto \
  -H "Host: grpc.example.com" \
  -d '{"name":"GatewayTest"}' \
  127.0.0.1:18080 \
  test.TestService/SayHello

if [ $? -eq 0 ]; then
    echo ""
    echo "✅ 测试 2 通过: Gateway 路由工作正常"
else
    echo ""
    echo "❌ 测试 2 失败: Gateway 路由失败"
    echo ""
    echo "可能的原因:"
    echo "1. Controller 没有推送 GRPCRoute 配置到 Gateway"
    echo "2. Gateway 监听器名称不匹配 (需要 http18080)"
    echo "3. Hostname 不匹配 (需要 grpc.example.com)"
    echo ""
    echo "检查 Gateway 日志查看详细错误信息"
    exit 1
fi

echo ""
echo "======================================"
echo "测试 3: 流式方法 (StreamNumbers)"
echo "======================================"
echo ""
grpcurl -plaintext \
  -proto examples/proto/test_service.proto \
  -H "Host: grpc.example.com" \
  -d '{"count":3}' \
  127.0.0.1:18080 \
  test.TestService/StreamNumbers

if [ $? -eq 0 ]; then
    echo ""
    echo "✅ 测试 3 通过: 流式方法工作正常"
else
    echo ""
    echo "⚠️  测试 3 失败: 流式方法失败"
fi

echo ""
echo "======================================"
echo "测试 4: 负载均衡 (10次请求)"
echo "======================================"
echo ""
echo "观察 serverAddr 分布..."
for i in {1..10}; do
    grpcurl -plaintext \
      -proto examples/proto/test_service.proto \
      -H "Host: grpc.example.com" \
      -d '{"name":"LB-Test-'$i'"}' \
      127.0.0.1:18080 \
      test.TestService/SayHello 2>/dev/null | grep serverAddr
done

echo ""
echo "======================================"
echo "🎉 测试完成！"
echo "======================================"
echo ""
echo "如果所有测试都通过，说明:"
echo "✅ gRPC Server 工作正常"
echo "✅ Gateway HTTP/2 (h2c) 支持正常"
echo "✅ GRPCRoute 路由匹配正常"
echo "✅ 负载均衡分配正常"
echo ""
echo "更多测试场景请查看: tests/start_cmd.txt"

