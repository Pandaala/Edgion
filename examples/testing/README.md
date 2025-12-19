# Edgion 测试工具

统一的测试服务器和客户端，支持所有协议测试。

## 快速开始

### 方式 1：一键集成测试（推荐）

使用集成测试脚本，自动启动所有服务并运行测试：

```bash
# 进入测试目录
cd examples/testing

# 运行集成测试脚本
./run_integration_test.sh
```

脚本会自动：
1. 生成 TLS 证书（自签名，用于 HTTPS 测试）
2. 启动 test_server（后端服务）
3. 启动 edgion-controller（加载 examples/conf 配置）
4. 启动 edgion-gateway（网关服务）
5. 等待 10 秒让服务完全启动
6. 运行 Direct 模式测试（HTTP, gRPC, TCP, UDP, WebSocket）
7. 运行 Gateway 模式测试（HTTP, gRPC, TCP, UDP, WebSocket, HTTPS, gRPC-HTTPS）
8. 显示测试结果和日志
9. 自动清理所有服务

### 方式 2：手动启动和测试

#### 1. 启动测试服务器

```bash
cargo run --example test_server
```

#### 2. 运行测试客户端

测试客户端支持两种模式：

**Direct 模式（默认，直连后端服务）**：
```bash
# 测试所有协议
cargo run --example test_client -- all

# 测试单个协议
cargo run --example test_client -- http

# 自定义端口
cargo run --example test_client -- --http-port 30001 http
```

**Gateway 模式（通过 Gateway 代理）**：
```bash
# 使用 -g flag 启用 Gateway 模式
cargo run --example test_client -- -g http

# 或使用长参数
cargo run --example test_client -- --gateway http

# Gateway 模式会自动：
# - 使用 Gateway 端口（HTTP: 10080, HTTPS: 18443, gRPC: 10080, TCP: 19000, UDP: 19002）
# - 设置 Host header 为 test.example.com（HTTP/HTTPS 路由匹配需要）
# - 设置 Authority header 为 grpc.example.com（gRPC 路由匹配需要）

# HTTPS 测试（仅 Gateway 模式）
cargo run --example test_client -- -g https

# gRPC-HTTPS 测试（仅 Gateway 模式）
cargo run --example test_client -- -g grpc-https
```

## 默认端口

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
| HTTPS | 18443 | HTTPS 网关（TLS termination）|
| gRPC | 10080 | gRPC 网关（HTTP/2）|
| gRPC-HTTPS | 18443 | gRPC 网关（HTTPS/HTTP2）|
| TCP | 19000 | TCP 代理 |
| UDP | 19002 | UDP 代理 |

## 测试命令

### Direct 模式示例

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

### Gateway 模式示例

```bash
# 测试 HTTP（通过 Gateway :10080）
cargo run --example test_client -- -g http

# 测试 HTTPS（通过 Gateway :18443，仅 Gateway 模式）
cargo run --example test_client -- -g https

# 测试 gRPC（通过 Gateway :10080）
cargo run --example test_client -- -g grpc

# 测试 gRPC-HTTPS（通过 Gateway :18443，仅 Gateway 模式）
cargo run --example test_client -- -g grpc-https

# 测试所有协议（通过 Gateway，包括 HTTPS）
cargo run --example test_client -- -g all

# Gateway 模式 + 详细输出
cargo run --example test_client -- -g --verbose http

# Gateway 模式 + JSON 报告
cargo run --example test_client -- -g --json http
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

## TLS 证书生成

HTTPS 和 gRPC-HTTPS 测试需要 TLS 证书。集成测试脚本会自动生成证书，也可以手动生成：

```bash
# 手动生成 TLS 证书
cd examples/testing
./scripts/generate_certs.sh

# 如果 Secret 文件已存在，脚本会自动跳过生成
# 需要重新生成时，先删除 Secret 文件：
rm ../conf/Secret_edge_tls.yaml
./scripts/generate_certs.sh
```

证书生成说明：
- **智能跳过**：如果 `Secret_edge_tls.yaml` 已存在，脚本会自动跳过生成（提高效率）
- **按需生成**：删除 Secret 文件后重新运行脚本可强制重新生成
- 证书临时生成在 `/tmp/edgion-certs-$$` 目录
- 生成一个包含多个域名的自签名证书（SAN）：
  - `test.example.com`（用于 HTTPS 测试）
  - `grpc.example.com`（用于 gRPC-HTTPS 测试）
- 证书自动转换为 Kubernetes Secret YAML 格式
- Secret YAML 保存到 `examples/conf/Secret_edge_tls.yaml`
- 临时证书文件自动清理
- Secret YAML 被 `.gitignore` 忽略，不会提交到 git

生成的资源：
```
examples/conf/
├── Secret_edge_edge-tls.yaml       # TLS 证书 Secret（包含多个域名）
└── EdgionTls_edge_edge-tls.yaml    # TLS 证书配置（绑定到 HTTPS 和 gRPC-HTTPS）
```

## 日志文件

集成测试脚本会将日志保存到 `examples/testing/logs/` 目录：

```
examples/testing/logs/
├── controller.log    # edgion-controller 日志
├── gateway.log       # edgion-gateway 日志
├── test_server.log   # test_server 日志
├── access.log        # HTTP 访问日志
└── test_result.log   # 测试结果日志
```

查看日志：
```bash
# 查看 Gateway 访问日志
tail -f examples/testing/logs/access.log

# 查看测试结果
cat examples/testing/logs/test_result.log

# 查看所有日志
ls -lh examples/testing/logs/
```


运行gateway和controller:
cargo run --bin edgion-gateway
cargo run --bin edgion-controller