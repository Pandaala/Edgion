# Edgion 集成测试配置

本目录包含集成测试所需的 Kubernetes Gateway API 配置文件。

## 目录结构

```
conf/
├── base/                   # 基础配置（所有测试共用）
│   ├── GatewayClass.yaml
│   ├── Gateway.yaml
│   ├── EdgionGatewayConfig.yaml
│   ├── EdgionTls_edge_edge-tls.yaml
│   └── Secret_edgion-test_edge-tls.yaml
├── http/                   # HTTP 基础测试
├── grpc/                   # gRPC 基础测试
├── tcp/                    # TCP 测试
├── udp/                    # UDP 测试
├── http-match/             # HTTP 匹配规则测试
├── grpc-match/             # gRPC 匹配规则测试
├── grpc-tls/               # gRPC TLS 测试
├── lb-policy/              # 负载均衡策略测试
├── weighted-backend/       # 权重后端测试
├── timeout/                # 超时测试
├── plugins/                # 插件测试
├── redirect/               # HTTP 重定向测试
├── stream-plugins/         # 流式插件测试
├── mtls/                   # 双向 TLS (mTLS) 测试
└── backend-tls/            # 后端 TLS 测试
```

## 加载顺序

1. **base/** - 必须首先加载，包含 GatewayClass、Gateway 等基础资源
2. **各 suite 目录** - 按需加载测试所需的配置

## 使用方法

```bash
# 使用 start_all_with_conf.sh 启动并加载配置
./examples/test/scripts/utils/start_all_with_conf.sh

# 启动并只加载特定 suite
./examples/test/scripts/utils/start_all_with_conf.sh --suites http,grpc

# 单独加载配置（需要 controller 已运行）
./examples/test/scripts/utils/load_conf.sh http

# 加载所有配置
./examples/test/scripts/utils/load_conf.sh all
```

## 配置说明

### base/

- **GatewayClass.yaml**: 定义 Gateway 类型，关联 EdgionGatewayConfig
- **Gateway.yaml**: 定义监听器（HTTP:10080, HTTPS:10443, TCP:19000, UDP:19002, gRPC:18443）
- **EdgionGatewayConfig.yaml**: Edgion 特定配置（超时、Real IP、安全防护等）
- **EdgionTls_edge_edge-tls.yaml**: TLS 配置，关联 Secret
- **Secret_edgion-test_edge-tls.yaml**: TLS 证书 Secret

### http/

- **Service_test-http.yaml**: HTTP 测试服务定义
- **EndpointSlice_test-http.yaml**: 后端服务发现（指向 127.0.0.1:30001）
- **HTTPRoute.yaml**: HTTP 路由规则（Host: test.example.com）
- **Service_test-websocket.yaml**: WebSocket 服务定义
- **EndpointSlice_test-websocket.yaml**: WebSocket 后端服务发现

### http-match/

- **HTTPRoute_default_match-test.yaml**: HTTP 匹配规则测试路由
- **HTTPRoute_section-test.yaml**: Section name 匹配测试
- **HTTPRoute_wildcard.yaml**: 通配符主机名测试

### grpc/

- **Service_test-grpc.yaml**: gRPC 服务定义
- **EndpointSlice_test-grpc.yaml**: gRPC 后端服务发现
- **GRPCRoute.yaml**: gRPC 路由规则

### grpc-match/

- **GRPCRoute_edge_match-test.yaml**: gRPC 匹配规则测试
- **GRPCRoute_edge_match-test-wrong-section.yaml**: 错误 section 测试

### grpc-tls/

- **GRPCRoute_edge_test-grpc-https.yaml**: gRPC over TLS 路由

### lb-policy/

- **Service_default_lb-rr-test.yaml**: 负载均衡服务定义
- **EndpointSlice_default_lb-rr-test.yaml**: 多后端服务发现
- **HTTPRoute_default_lb-rr-noretry.yaml**: 轮询负载均衡路由

### weighted-backend/

- **Service_edge_backend-*.yaml**: 权重后端服务定义
- **EndpointSlice_edge_backend-*.yaml**: 多后端服务发现
- **HTTPRoute_default_weighted-backend.yaml**: 权重路由规则

### timeout/

- **EdgionPlugins_default_timeout-debug.yaml**: 超时调试插件
- **HTTPRoute_default_timeout-backend.yaml**: 超时测试路由

### mtls/

- **Gateway_edge_mtls-test-gateway.yaml**: mTLS 测试网关
- **EdgionTls_edge_mtls-test-*.yaml**: 各种 mTLS 配置
- **HTTPRoute_edge_mtls-test.yaml**: mTLS 测试路由

### backend-tls/

- **BackendTLSPolicy_edge_backend-tls.yaml**: 后端 TLS 策略
- **Service_edge_test-backend-tls.yaml**: 后端 TLS 服务
- **EndpointSlice_edge_test-backend-tls.yaml**: 后端 TLS 服务发现
- **HTTPRoute_edge_backend-tls.yaml**: 后端 TLS 路由
- **Secret_backend-ca.yaml**: 后端 CA 证书

## 添加新 suite

1. 创建对应目录
2. 添加所需配置文件（Service、EndpointSlice、Route 等）
3. 配置会自动被扫描和加载（目录名即为 suite 名）
