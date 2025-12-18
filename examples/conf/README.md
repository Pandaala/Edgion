# Edgion 配置示例

用于测试的网关配置文件。

## 配置文件

- `GatewayClass__public-gateway.yaml` - GatewayClass
- `Gateway_edge_example-gateway.yaml` - Gateway（HTTP/HTTPS/gRPC/TCP）
- `HTTPRoute_edge_test-http.yaml` - HTTP 路由
- `GRPCRoute_edge_test-grpc.yaml` - gRPC 路由
- `TCPRoute_edge_test-tcp.yaml` - TCP 路由
- `Service_*.yaml` / `EndpointSlice_*.yaml` - 后端服务

## 手动启动服务

```bash
# 1. 启动后端测试服务（30001-30004, 30021 等端口）
cargo run --example test_server

# 2. 启动 controller（使用默认配置 config/edgion-controller.toml）
cargo run --bin edgion-controller

# 3. 启动 gateway（使用默认配置 config/edgion-gateway.toml）
cargo run --bin edgion-gateway
```

## 手动测试

### HTTP 测试（curl）

```bash
# 健康检查
curl -H "Host: test.example.com" http://localhost:10080/health

# Echo 测试
curl -H "Host: test.example.com" http://localhost:10080/echo

# 延迟测试
curl -H "Host: test.example.com" http://localhost:10080/delay/1

# API 测试
curl -H "Host: test.example.com" http://localhost:10080/api/users
```

### gRPC 测试（grpcurl）

```bash
# Gateway 模式
grpcurl -plaintext -authority grpc.example.com localhost:18443 test.TestService/SayHello

# Direct 模式
grpcurl -plaintext localhost:30021 test.TestService/SayHello
```

### TCP 测试（nc/telnet）

```bash
# Gateway 模式（通过 19000）
echo "Hello TCP" | nc localhost 19000

# Direct 模式（直连 30010）
echo "Hello TCP" | nc localhost 30010
```
