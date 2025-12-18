# Edgion 测试工具

统一的测试服务器和客户端，支持所有协议测试。

## 快速开始

### 1. 启动测试服务器

```bash
cargo run --example test_server
```

### 2. 运行测试客户端

```bash
# 在另一个终端运行
cargo run --example test_client -- all
```

## 默认端口

test-server 默认监听以下端口：

| 协议 | 端口 | 说明 |
|------|------|------|
| HTTP | 30001 | HTTP 测试服务 |
| gRPC | 30021 | gRPC 测试服务 |
| WebSocket | 30005 | WebSocket 回显服务 |
| TCP | 30010 | TCP 回显服务 |
| UDP | 30011 | UDP 回显服务 |

## 测试命令

```bash
# 测试所有协议
cargo run --example test_client -- all

# 测试单个协议
cargo run --example test_client -- http
cargo run --example test_client -- tcp
cargo run --example test_client -- udp

# 详细输出
cargo run --example test_client -- --verbose all

# 生成 JSON 报告
cargo run --example test_client -- --json all
```

## 自定义端口

```bash
# test-server 自定义端口
cargo run --example test_server -- \
  --http-ports 30001 \
  --grpc-ports 30021 \
  --tcp-port 30010

# test-client 连接自定义端口
cargo run --example test_client -- \
  --http-port 30001 \
  --tcp-port 30010 \
  all
```

