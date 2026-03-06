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
├── lb-roundrobin/          # RoundRobin 负载均衡测试
├── lb-consistenthash/      # ConsistentHash 一致性哈希测试
├── weighted-backend/       # 权重后端测试
├── timeout/                # 超时测试
├── plugins/                # 插件测试
├── redirect/               # HTTP 重定向测试
├── stream-plugins/         # 流式插件测试
├── mtls/                   # 双向 TLS (mTLS) 测试
├── backend-tls/            # 后端 TLS 测试
└── EdgionTls/
    └── cipher/             # TLS cipher 加密算法测试
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

### lb-roundrobin/

RoundRobin 轮询负载均衡测试，验证请求均匀分布到所有后端。

- **Gateway.yaml**: RoundRobin 测试网关（端口 31120）
- **Service_default_lb-rr.yaml**: 负载均衡服务定义
- **EndpointSlice_default_lb-rr.yaml**: 单 slice 后端（3 endpoints）
- **Endpoints_default_lb-rr.yaml**: Endpoints 资源（用于 EP 模式测试）
- **HTTPRoute_default_lb-rr-eps.yaml**: EndpointSlice 模式路由
- **HTTPRoute_default_lb-rr-ep.yaml**: Endpoints 模式路由（kind: ServiceEndpoint）
- **Service_default_lb-rr-multi.yaml**: 多 slice 测试服务
- **EndpointSlice_default_lb-rr-multi-1/2.yaml**: 多 slice 后端（2 slices, 4 endpoints）
- **HTTPRoute_default_lb-rr-multi.yaml**: 多 slice 测试路由

测试场景：
1. EndpointSlice 模式（默认）- 单 slice，验证轮询分布
2. Endpoints 模式 - 使用 ServiceEndpoint kind
3. 多 slice 模式 - 跨多个 EndpointSlice 的轮询

### lb-consistenthash/

ConsistentHash 一致性哈希测试，验证相同 key 始终路由到相同后端。

- **Gateway.yaml**: ConsistentHash 测试网关（端口 31121）
- **Service_default_lb-ch.yaml**: 服务定义
- **EndpointSlice_default_lb-ch.yaml**: 单 slice 后端
- **Endpoints_default_lb-ch.yaml**: Endpoints 资源
- **HTTPRoute_default_lb-ch-header-eps.yaml**: Header 哈希 + EndpointSlice
- **HTTPRoute_default_lb-ch-header-ep.yaml**: Header 哈希 + Endpoints
- **HTTPRoute_default_lb-ch-cookie.yaml**: Cookie 哈希
- **HTTPRoute_default_lb-ch-arg.yaml**: Query 参数哈希
- **Service_default_lb-ch-multi.yaml**: 多 slice 测试服务
- **EndpointSlice_default_lb-ch-multi-1/2.yaml**: 多 slice 后端
- **HTTPRoute_default_lb-ch-multi.yaml**: 多 slice 一致性哈希

测试场景：
1. Header 哈希 - 基于 x-user-id header
2. Cookie 哈希 - 基于 session-id cookie
3. Query 参数哈希 - 基于 user_id 参数
4. EPS vs EP 模式对比
5. 多 slice 一致性验证

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

### EdgionTls/cipher/

TLS cipher（加密算法）配置测试，验证 EdgionTls 在 TLS 1.2 时支持自定义 cipher 列表。

- **Gateway.yaml**: cipher 测试网关（端口 31195/31196）
- **HTTPRoute.yaml**: cipher 测试路由
- **EdgionTls_cipher_legacy.yaml**: TLS 1.2 + 弱算法配置（AES128-SHA, AES256-SHA 等）
- **EdgionTls_cipher_modern.yaml**: TLS 1.2 + 现代算法配置（ECDHE-RSA-AES256-GCM-SHA384 等）

测试用例使用 `openssl s_client` 验证服务端协商的 cipher 是否符合配置。

### backend-tls/

- **BackendTLSPolicy_edge_backend-tls.yaml**: 后端 TLS 策略
- **Service_edge_test-backend-tls.yaml**: 后端 TLS 服务
- **EndpointSlice_edge_test-backend-tls.yaml**: 后端 TLS 服务发现
- **HTTPRoute_edge_backend-tls.yaml**: 后端 TLS 路由
- **Secret_backend-ca.yaml**: 后端 CA 证书

### ref-grant-status/

ReferenceGrant 和 Status 系统集成测试，验证跨命名空间引用的 status 更新。

- **Service_backend_cross-ns-svc.yaml**: backend 命名空间的服务
- **EndpointSlice_backend_cross-ns-svc.yaml**: 后端服务发现
- **HTTPRoute_app_cross-ns-route.yaml**: `edgion-default` 命名空间的 HTTPRoute，跨命名空间引用 `edgion-backend` 的 Service
- **HTTPRoute_app_cross-ns-denied.yaml**: `edgion-default` 命名空间的 HTTPRoute，跨命名空间引用 `edgion-system` 的 Service（无 ReferenceGrant，应被拒绝）
- **HTTPRoute_app_multi-parent.yaml**: `edgion-default` 命名空间的多 parentRefs 测试
- **ReferenceGrant_backend_allow-app.yaml**: 允许 `edgion-default` 命名空间的 HTTPRoute 引用 `edgion-backend` 的 Service

说明：文件名里的 `app` 仅为历史命名，当前测试不再创建额外的 `app` / `other` namespace。

测试场景：
1. 跨命名空间引用 + 有 ReferenceGrant → ResolvedRefs=True
2. 跨命名空间引用 + 无 ReferenceGrant → ResolvedRefs=False (RefNotPermitted)
3. ReferenceGrant 后到 → HTTPRoute 自动 requeue 并更新 status
4. 多 parentRefs → 每个 parent 独立 status

## 添加新 suite

1. 创建对应目录
2. 添加所需配置文件（Service、EndpointSlice、Route 等）
3. 配置会自动被扫描和加载（目录名即为 suite 名）
