# Edgion 测试工具和配置

统一的测试服务器、客户端和配置文件，支持所有协议测试。

## 目录

- [快速开始](#快速开始)
- [手动启动服务](#手动启动服务)
- [test_client 命令行](#test_client-命令行)
- [手动测试命令（curl/grpcurl/nc）](#手动测试命令)
- [端口说明](#端口说明)
- [TLS 证书](#tls-证书)
- [日志文件](#日志文件)

---

## 快速开始

### 一键集成测试（推荐）

```bash
cd examples/testing
./run_integration_test.sh
```

脚本会自动：
1. 生成 TLS 证书（用于 HTTPS 测试）
2. 启动 test_server（后端服务）
3. 启动 edgion-controller（加载 `examples/conf` 配置）
4. 启动 edgion-gateway（网关服务）
5. 运行所有测试（Direct + Gateway 模式，包括 HTTPS）
6. 显示结果并清理服务

---

## 手动启动服务

### 1. 启动后端测试服务

```bash
cargo run --example test_server
```

启动的服务：
- HTTP: `30001-30004`（4 个实例）
- gRPC: `30021`
- WebSocket: `30005`
- TCP: `30010`
- UDP: `30011`

### 2. 启动 Controller

```bash
cargo run --bin edgion-controller
```

默认配置：
- gRPC 端口: `50051`
- 配置目录: `examples/conf`
- GatewayClass: `public-gateway`

### 3. 启动 Gateway

```bash
cargo run --bin edgion-gateway
```

启动的监听器：
- HTTP: `10080`
- HTTPS: `10443`
- gRPC (HTTP): `10080`
- gRPC (HTTPS): `18443` (手动测试)
- TCP: `19000`
- UDP: `19002`

---

## test_client 命令行

### 基本用法

```bash
# 语法
cargo run --example test_client -- [OPTIONS] <COMMAND>

# 选项
  -g, --gateway          # 启用 Gateway 模式（默认 Direct 模式）
  -v, --verbose          # 详细输出
  --json                 # JSON 格式报告
  --http-port <PORT>     # 自定义 HTTP 端口（Direct 模式，默认 30001）
  --grpc-port <PORT>     # 自定义 gRPC 端口（Direct 模式，默认 30021）
  --tcp-port <PORT>      # 自定义 TCP 端口（Direct 模式，默认 30010）
  --udp-port <PORT>      # 自定义 UDP 端口（Direct 模式，默认 30011）
  --websocket-port <PORT> # 自定义 WebSocket 端口（Direct 模式，默认 30005）
  --https-port <PORT>    # 自定义 HTTPS 端口（Gateway 模式，默认 10443）

# 命令
  http          # HTTP 测试
  grpc          # gRPC 测试
  tcp           # TCP 测试
  udp           # UDP 测试
  websocket     # WebSocket 测试
  https         # HTTPS 测试（仅 Gateway 模式）
  all           # 运行所有测试
```

### Direct 模式示例

```bash
# 测试所有协议（直连后端）
cargo run --example test_client -- all

# 测试单个协议
cargo run --example test_client -- http
cargo run --example test_client -- grpc
cargo run --example test_client -- tcp
cargo run --example test_client -- udp
cargo run --example test_client -- websocket

# 详细输出
cargo run --example test_client -- --verbose http

# 自定义端口
cargo run --example test_client -- --http-port 8080 http
```

### Gateway 模式示例

```bash
# 测试所有协议（通过 Gateway）
cargo run --example test_client -- -g all

# 测试单个协议
cargo run --example test_client -- -g http      # HTTP (10080)
cargo run --example test_client -- -g grpc      # gRPC (10080, HTTP/2)
cargo run --example test_client -- -g tcp       # TCP (19000)
cargo run --example test_client -- -g udp       # UDP (19002)
cargo run --example test_client -- -g websocket # WebSocket (10080)

# HTTPS 测试（仅 Gateway 模式）
cargo run --example test_client -- -g https     # HTTPS (10443)

# Gateway 模式 + 详细输出
cargo run --example test_client -- -g --verbose http

# Gateway 模式 + JSON 报告
cargo run --example test_client -- -g --json all
```

**Gateway 模式特点：**
- 自动使用 Gateway 端口
- 自动设置 `Host: test.example.com`（HTTP/HTTPS 路由匹配需要）
- 自动设置 `Authority: grpc.example.com`（gRPC 路由匹配需要）

---

## 手动测试命令

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

### HTTPS 测试（curl）

```bash
# 健康检查（HTTPS）
curl -k -H "Host: test.example.com" \
  --resolve test.example.com:10443:127.0.0.1 \
  https://test.example.com:10443/secure/health

# Echo 测试（HTTPS）
curl -k -H "Host: test.example.com" \
  --resolve test.example.com:10443:127.0.0.1 \
  https://test.example.com:10443/secure/echo

# Status 测试（HTTPS）
curl -k -H "Host: test.example.com" \
  --resolve test.example.com:10443:127.0.0.1 \
  https://test.example.com:10443/secure/status/200
```

**说明：**
- `-k` 跳过证书验证（自签名证书）
- `--resolve` 将域名解析到 127.0.0.1
- 必须带 `-H "Host: test.example.com"` 和 `--resolve`

### gRPC 测试（grpcurl）

```bash
# Gateway HTTP 模式（10080）
grpcurl -plaintext -authority grpc.example.com \
  -proto examples/proto/test_service.proto \
  -import-path . \
  localhost:10080 test.TestService/SayHello

# Gateway HTTP 模式（带参数）
grpcurl -plaintext -authority grpc.example.com \
  -proto examples/proto/test_service.proto \
  -import-path . \
  -d '{"name": "World"}' \
  localhost:10080 test.TestService/SayHello

# Gateway HTTPS 模式（18443）
grpcurl -insecure -authority grpc.example.com \
  -proto examples/proto/test_service.proto \
  -import-path . \
  localhost:18443 test.TestService/SayHello

# Direct 模式（30021）
grpcurl -plaintext \
  -proto examples/proto/test_service.proto \
  -import-path . \
  localhost:30021 test.TestService/SayHello
```

**说明：**
- `-proto` 指定 proto 文件路径（test_server 未启用 reflection API）
- `-import-path .` proto 文件导入路径
- `-authority grpc.example.com` 设置 `:authority` 伪头部（Gateway 模式必需）
- `-d '{"name": "World"}'` 请求参数（JSON 格式）

### TCP 测试（nc/telnet）

```bash
# Gateway 模式（19000）
(echo "Hello TCP"; sleep 0.5) | nc localhost 19000

# 或使用 -q 参数（部分系统支持）
echo "Hello TCP" | nc -q 1 localhost 19000

# Direct 模式（30010）
(echo "Hello TCP"; sleep 0.5) | nc localhost 30010
```

**说明：**
- `sleep 0.5` 让连接保持打开以接收响应
- `-q 1` 在 EOF 后等待 1 秒再关闭（GNU netcat）

### UDP 测试（nc）

```bash
# Gateway 模式（19002）
echo "Hello UDP" | nc -u localhost 19002

# Direct 模式（30011）
echo "Hello UDP" | nc -u localhost 30011
```

### WebSocket 测试

```bash
# 使用 test_client（推荐）
cargo run --example test_client -- -g websocket  # Gateway 模式
cargo run --example test_client -- websocket     # Direct 模式

# 使用 websocat 手动测试
echo "Hello WebSocket" | websocat ws://localhost:10080/ws  # Gateway
echo "Hello WebSocket" | websocat ws://localhost:30005/ws  # Direct
```

---

## 端口说明

### test_server 后端端口

| 协议 | 端口 | 说明 |
|------|------|------|
| HTTP | 30001-30004 | HTTP 测试服务（4 个实例）|
| gRPC | 30021 | gRPC 测试服务 |
| WebSocket | 30005 | WebSocket 回显服务 |
| TCP | 30010 | TCP 回显服务 |
| UDP | 30011 | UDP 回显服务 |

### Gateway 监听端口

| 协议 | 端口 | 说明 |
|------|------|------|
| HTTP | 10080 | HTTP 网关 |
| HTTPS | 10443 | HTTPS 网关（TLS termination）|
| gRPC | 10080 | gRPC 网关（HTTP/2）|
| gRPC-HTTPS | 18443 | gRPC 网关（HTTPS/HTTP/2，手动测试）|
| TCP | 19000 | TCP 代理 |
| UDP | 19002 | UDP 代理 |

### Controller 端口

| 端口 | 说明 |
|------|------|
| 50051 | gRPC API（Gateway 连接）|
| 8080 | Admin API |

---

## TLS 证书

HTTPS 和 gRPC-HTTPS 测试需要 TLS 证书。

### 自动生成（推荐）

```bash
cd examples/testing
./scripts/generate_certs.sh
```

### 生成规则

- **智能跳过**：如果 `Secret_edge_edge-tls.yaml` 已存在，自动跳过
- **按需重新生成**：
  ```bash
  rm ../conf/Secret_edge_edge-tls.yaml
  ./scripts/generate_certs.sh
  ```

### 证书说明

- 自签名证书（仅用于测试）
- 支持多个域名（SAN）：
  - `test.example.com`（HTTPS 测试）
  - `grpc.example.com`（gRPC-HTTPS 测试）
- 临时文件自动清理（`/tmp/edgion-certs-$$`）
- Secret YAML 被 `.gitignore` 忽略

### 生成的资源

```
examples/conf/
├── Secret_edge_edge-tls.yaml     # TLS 证书 Secret
└── EdgionTls_edge_edge-tls.yaml  # TLS 证书配置
```

---

## 日志文件

集成测试脚本日志位置：`examples/testing/logs/`

```
examples/testing/logs/
├── controller.log    # edgion-controller 日志
├── gateway.log       # edgion-gateway 日志
├── test_server.log   # test_server 日志
├── access.log        # HTTP 访问日志
└── test_result.log   # 测试结果日志
```

### 查看日志

```bash
# 实时查看 Gateway 访问日志
tail -f examples/testing/logs/access.log

# 查看测试结果
cat examples/testing/logs/test_result.log

# 查看 Gateway 日志
tail -f examples/testing/logs/gateway.log

# 查看所有日志
ls -lh examples/testing/logs/
```

---

## 配置文件

配置文件位于 `examples/conf/`：

### Gateway API 资源

- `GatewayClass__public-gateway.yaml` - GatewayClass
- `Gateway_edge_example-gateway.yaml` - Gateway（HTTP/HTTPS/gRPC/TCP/UDP）
- `HTTPRoute_edge_test-http.yaml` - HTTP 路由（包含 WebSocket）
- `GRPCRoute_edge_test-grpc.yaml` - gRPC HTTP 路由（10080）
- `GRPCRoute_edge_test-grpc-https.yaml` - gRPC HTTPS 路由（18443）
- `TCPRoute_edge_test-tcp.yaml` - TCP 路由
- `UDPRoute_edge_test-udp.yaml` - UDP 路由

### 后端服务

- `Service_edge_test-*.yaml` - Service 定义
- `EndpointSlice_edge_test-*.yaml` - 后端 Endpoint

### TLS 资源

- `EdgionTls_edge_edge-tls.yaml` - TLS 证书配置
- `Secret_edge_edge-tls.yaml` - TLS 证书数据（自动生成，被 gitignore）

---

## 故障排查

### Gateway 启动失败

```bash
# 检查 Controller 是否在运行
ps aux | grep edgion-controller

# 检查 Controller 端口
lsof -i :50051

# 查看 Gateway 日志
tail -100 examples/testing/logs/gateway.log
```

### HTTPS 测试失败

```bash
# 检查证书是否生成
ls examples/conf/Secret_edge_edge-tls.yaml

# 重新生成证书
rm examples/conf/Secret_edge_edge-tls.yaml
./scripts/generate_certs.sh

# 检查 HTTPS 监听器
lsof -i :10443
```

### 测试连接失败

```bash
# 检查所有服务进程
ps aux | grep -E "edgion|test_server"

# 检查端口占用
lsof -i :10080  # HTTP Gateway
lsof -i :10443  # HTTPS Gateway
lsof -i :30001  # HTTP Backend
```
