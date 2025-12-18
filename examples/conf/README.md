# Edgion 配置示例

网关配置示例文件，用于快速测试。

## 文件列表

### 网关配置
- `EdgionGatewayConfig__example-gateway.yaml` - 网关全局配置
- `Gateway_edge_example-gateway.yaml` - Gateway 示例，包含多协议监听器（HTTP/HTTPS/gRPC/TLS/TCP/UDP）

### HTTP 路由相关
- `HTTPRoute_edge_test-http.yaml` - HTTP 路由配置示例
  - `/api/*` → test-http:30001（路径前缀匹配）
  - `/health` → test-http:30001（精确路径匹配）
- `Service_edge_test-http.yaml` - test-http 服务（端口 30001）
- `EndpointSlice_edge_test-http.yaml` - test-http 服务端点（127.0.0.1:30001）

## 使用方法

```bash
# 启动 controller（加载配置）
cargo run --bin edgion-controller -- \
  --gateway-class example-gateway \
  --loader-dir examples/conf

# 启动 gateway
cargo run --bin edgion-gateway -- \
  --gateway-class example-gateway
```

## 配合测试

### 启动后端测试服务

```bash
# 启动 test_http_server（默认监听 30001 等端口）
cargo run --example test_http_server
```

### HTTP 路由测试

**方法 1：使用 test_client（推荐）**

```bash
# Gateway 模式测试（自动设置 Host header）
cargo run --example test_client -- -g http

# Direct 模式测试（直连后端）
cargo run --example test_client -- http
```

**方法 2：使用 curl 手动测试**

```bash
# 测试 API 路径前缀匹配
curl -H "Host: test.example.com" http://localhost:10080/api/users
curl -H "Host: test.example.com" http://localhost:10080/api/v1/orders

# 测试健康检查端点（精确匹配）
curl -H "Host: test.example.com" http://localhost:10080/health
```

### 验证 access.log

测试后检查访问日志，确认请求被正确路由和记录：
```bash
tail -f logs/edgion_access.log
```
