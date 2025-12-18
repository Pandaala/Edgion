# gRPC Routes 测试配置

本目录包含了用于测试 gRPC 路由功能的配置文件。

## 测试文件列表

### 1. GRPCRoute 配置
**文件:** `GRPCRoute_test1_grpc-route1.yaml`

包含 6 个测试规则，涵盖了不同的匹配场景：

1. **精确匹配服务和方法**
   - 匹配: `helloworld.Greeter/SayHello`
   - 用途: 测试最精确的路由规则

2. **匹配服务的所有方法**
   - 匹配: `helloworld.Greeter/*`
   - 用途: 测试服务级别的路由规则

3. **正则表达式匹配 + Header 过滤**
   - 匹配: `*.UserService/Get*` + `x-api-version: v1`
   - 用途: 测试高级匹配和负载均衡（70%主 + 30%备）

4. **负载均衡测试**
   - 匹配: `product.ProductService/*`
   - 权重: 70% 主服务器 + 30% 备份服务器

5. **插件集成测试**
   - 匹配: `auth.AuthService/Login`
   - 用途: 测试与 EdgionPlugins 的集成

6. **通配符（兜底规则）**
   - 匹配: 所有服务
   - 用途: 作为默认路由

### 2. Service 配置
**文件:** `Service_test1_grpc-service.yaml`

包含两个 Kubernetes Service：
- `grpc-service` (port: 50051) - 主 gRPC 服务
- `grpc-service-backup` (port: 50052) - 备份 gRPC 服务

### 3. EndpointSlice 配置
**文件:** `EndpointSlice_test1_grpc-service.yaml`

为两个 Service 提供后端端点：
- 都指向 `127.0.0.1`（本地测试）
- 可根据实际情况修改地址

## 快速开始

### 前置条件

1. **Edgion Gateway 已运行**
   ```bash
   cd /Users/caohao/code/Edgion
   cargo build --release
   ./target/release/edgion-gateway --config config/edgion-gateway.toml
   ```

2. **Gateway 资源已创建**
   - 确保 `gateway1` 存在于 `default` namespace
   - 配置了 `https18443` sectionName

3. **启动测试 gRPC 服务器**
   ```bash
   # 使用 examples 中的测试服务器
   cd /Users/caohao/code/Edgion
   cargo run --example test_grpc_server -- --port 50051
   
   # 在另一个终端启动备份服务器
   cargo run --example test_grpc_server -- --port 50052
   ```

### 应用配置

```bash
# 应用 GRPCRoute 配置
kubectl apply -f config/examples/GRPCRoute_test1_grpc-route1.yaml

# 应用 Service 配置
kubectl apply -f config/examples/Service_test1_grpc-service.yaml

# 应用 EndpointSlice 配置
kubectl apply -f config/examples/EndpointSlice_test1_grpc-service.yaml
```

### 测试方法

#### 方法 1: 使用 grpcurl

```bash
# 测试精确匹配
grpcurl -plaintext \
  -H "Host: grpc.example.com" \
  -d '{"name": "World"}' \
  localhost:8443 \
  helloworld.Greeter/SayHello
```

#### 方法 2: 使用测试客户端

```bash
cd /Users/caohao/code/Edgion
cargo run --example test_grpc_client -- \
  --host grpc.example.com:8443 \
  --service helloworld.Greeter \
  --method SayHello \
  --data '{"name": "World"}'
```

#### 方法 3: gRPC-Web 测试

gRPC-Web 请求会被自动转换为标准 gRPC 请求：

```bash
curl -X POST https://grpc.example.com:8443/helloworld.Greeter/SayHello \
  -H "Content-Type: application/grpc-web" \
  -H "Host: grpc.example.com" \
  -d '<base64-encoded-grpc-web-frame>'
```

## 验证结果

### 1. 查看访问日志

```bash
tail -f logs/edgion_access.log | jq '.'
```

期望看到的字段：
```json
{
  "host": "grpc.example.com",
  "path": "/helloworld.Greeter/SayHello",
  "discover_protocol": "grpc",
  "grpc_service": "helloworld.Greeter",
  "grpc_method": "SayHello",
  "status": 200,
  "backend_addr": "127.0.0.1:50051"
}
```

### 2. 查看网关日志

```bash
tail -f logs/edgion-gateway.$(date +%Y-%m-%d) | grep grpc
```

期望看到的日志消息：
- `gRPC route matched`
- `Selected gRPC backend`
- `gRPC-Web bridge initialized` (如果是 gRPC-Web 请求)

### 3. 检查路由匹配

日志中应该显示：
```
[INFO] Matched gRPC route: grpc-route1, rule: 0, backend: grpc-service:50051
```

### 4. 负载均衡验证

运行多次请求，统计分布：
```bash
for i in {1..100}; do
  grpcurl -plaintext \
    -H "Host: grpc.example.com" \
    localhost:8443 \
    product.ProductService/ListProducts \
    2>&1 | grep -o "127.0.0.1:5005[12]"
done | sort | uniq -c
```

期望结果（大约）：
```
  70 127.0.0.1:50051
  30 127.0.0.1:50052
```

## 测试场景清单

- [ ] **基本路由**
  - [ ] 精确服务+方法匹配
  - [ ] 服务级别匹配（所有方法）
  - [ ] 正则表达式匹配

- [ ] **高级功能**
  - [ ] Header 过滤
  - [ ] 负载均衡（权重分配）
  - [ ] 插件集成（EdgionPlugins）

- [ ] **协议支持**
  - [ ] 标准 gRPC (HTTP/2)
  - [ ] gRPC-Web (HTTP/1.1 或 HTTP/2)
  - [ ] 协议自动检测

- [ ] **错误处理**
  - [ ] 路由未匹配（fallback to HTTP routes）
  - [ ] Backend 不可用（503）
  - [ ] 无效的 gRPC 路径格式（400）

- [ ] **性能测试**
  - [ ] 高并发场景
  - [ ] gRPC-Web 转换开销
  - [ ] 路由匹配性能

## 故障排查

### 问题 1: 路由不匹配

**症状:** 返回 404 或路由到 HTTP routes

**检查:**
1. Hostname 是否正确 (`grpc.example.com`)
2. gRPC path 格式是否正确 (`/{service}/{method}`)
3. Content-Type 是否为 `application/grpc` 或 `application/grpc-web`

**日志:**
```bash
grep "gRPC request but no gRPC route matched" logs/edgion-gateway.*
```

### 问题 2: Backend 连接失败

**症状:** 返回 503

**检查:**
1. Backend 服务是否在运行
2. EndpointSlice 地址是否正确
3. 端口是否可访问

**测试:**
```bash
nc -zv 127.0.0.1 50051
```

### 问题 3: gRPC-Web 转换失败

**症状:** gRPC-Web 请求返回错误

**检查:**
1. `GrpcWeb` 模块是否已初始化（查看日志）
2. Content-Type 是否正确
3. 请求 body 是否是有效的 gRPC-Web 格式

**日志:**
```bash
grep "GrpcWebBridge" logs/edgion-gateway.*
```

## 进阶测试

### 1. 压力测试

使用 ghz 进行压力测试：

```bash
ghz --insecure \
  --proto=examples/proto/helloworld.proto \
  --call=helloworld.Greeter.SayHello \
  -d '{"name":"World"}' \
  -n 10000 \
  -c 50 \
  grpc.example.com:8443
```

### 2. 双向流测试

测试流式 gRPC：

```bash
grpcurl -plaintext \
  -H "Host: grpc.example.com" \
  -d @ \
  localhost:8443 \
  helloworld.Greeter/StreamingCall \
  << EOF
{"name":"Message1"}
{"name":"Message2"}
{"name":"Message3"}
EOF
```

### 3. mTLS 测试

配置客户端证书：

```bash
grpcurl \
  -cacert ca.crt \
  -cert client.crt \
  -key client.key \
  -H "Host: grpc.example.com" \
  -d '{"name":"World"}' \
  grpc.example.com:8443 \
  helloworld.Greeter/SayHello
```

## 相关文档

- [GRPC_TEST_GUIDE.md](./GRPC_TEST_GUIDE.md) - 详细测试指南
- [../docs/resource-architecture-overview.md](../docs/resource-architecture-overview.md) - 架构概览
- [Gateway API GRPCRoute Spec](https://gateway-api.sigs.k8s.io/reference/spec/#gateway.networking.k8s.io/v1.GRPCRoute) - 官方规范

## 反馈

如有问题或建议，请在项目中提交 issue。

