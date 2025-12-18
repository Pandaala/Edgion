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
1. 启动 test_server（后端服务）
2. 启动 edgion-controller（加载 examples/conf 配置）
3. 启动 edgion-gateway（网关服务）
4. 等待 10 秒让服务完全启动
5. 运行 Direct 模式测试
6. 运行 Gateway 模式测试
7. 显示测试结果和日志
8. 自动清理所有服务

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
# - 使用 Gateway 端口（HTTP: 10080, gRPC: 18443, TCP: 19000, UDP: 19002）
# - 设置 Host header 为 test.example.com（HTTP 路由匹配需要）
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
| HTTPS | 10443 | HTTPS 网关 |
| gRPC | 18443 | gRPC 网关（HTTPS/HTTP2）|
| TCP | 19000 | TCP 代理 |

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

# 测试 gRPC（通过 Gateway :18443）
cargo run --example test_client -- -g grpc

# 测试所有协议（通过 Gateway）
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