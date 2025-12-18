#!/bin/bash
# gRPC 快速测试脚本

echo "======================================"
echo "gRPC 快速测试"
echo "======================================"
echo ""

# 检查 gRPC server 是否在运行
if pgrep -f "test_grpc_server" > /dev/null; then
    echo "✅ gRPC Server 正在运行"
else
    echo "⚠️  gRPC Server 未运行，请先启动:"
    echo "   cargo run --example test_grpc_server"
    exit 1
fi

echo ""
echo "📝 测试配置:"
echo "   Server 1: http://127.0.0.1:30021"
echo "   Server 2: http://127.0.0.1:30023"
echo ""

# 测试 1: 直连测试
echo "==================== 测试 1: 直连 gRPC Server ===================="
echo "命令: cargo run --example test_grpc_client http://127.0.0.1:30021"
echo ""
cargo run --example test_grpc_client http://127.0.0.1:30021 2>&1 | head -25

echo ""
echo "==================== 测试完成 ===================="
echo ""
echo "📚 更多测试方法请查看:"
echo "   - tests/start_cmd.txt"
echo "   - config/examples/GRPC_TEST_GUIDE.md"
echo "   - tests/GRPC_TEST_SUMMARY.md"
echo ""

