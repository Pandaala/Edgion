# GRPCRoute Conformance Tests (5 tests)

## Core Tests (4 tests)

### 1. GRPCExactMethodMatching
- **Features**: Gateway, GRPCRoute
- **Manifest**: `grpcroute-exact-method-matching.yaml`
- **描述**: gRPC 精确方法匹配
- **测试逻辑**:
  1. 创建 GRPCRoute，规则基于 Service/Method 精确匹配
  2. 测试用例:
     - 请求 `grpc.testing.EchoTestService/Echo` → 路由到 backend-v1
     - 请求 `grpc.testing.EchoTestService/EchoTwo` → 路由到 backend-v2
     - 只匹配 Service (不指定 Method) → 匹配该 Service 的所有方法
  3. gRPC 方法格式: `/package.service/method`
  4. 精确匹配大小写敏感

### 2. GRPCRouteHeaderMatching
- **Features**: Gateway, GRPCRoute
- **Manifest**: `grpcroute-header-matching.yaml`
- **描述**: gRPC header (metadata) 匹配
- **测试逻辑**:
  1. 创建 GRPCRoute，规则基于 gRPC metadata (headers) 匹配
  2. 测试用例:
     - 请求带 metadata `version: one` → 路由到 backend-v1
     - 请求带 metadata `version: two` → 路由到 backend-v2
     - 多个 header 条件组合匹配
  3. Header name 大小写不敏感
  4. Header value 精确匹配

### 3. GRPCRouteListenerHostnameMatching
- **Features**: Gateway, GRPCRoute
- **Manifest**: `grpcroute-listener-hostname-matching.yaml`
- **描述**: gRPC 路由的监听器 hostname 匹配
- **测试逻辑**:
  1. Gateway 有多个 listener，不同 hostname
  2. GRPCRoute 绑定到特定 listener
  3. gRPC 请求通过 `:authority` header 匹配 hostname
  4. 验证请求路由到正确后端

### 4. GRPCRouteWeight
- **Features**: Gateway, GRPCRoute
- **Manifest**: `grpcroute-weight.yaml`
- **描述**: gRPC 加权路由
- **测试逻辑**:
  1. 创建 GRPCRoute，backendRefs 有多个后端和不同 weight
  2. 发送大量 gRPC 请求
  3. 验证流量按 weight 比例分配
  4. weight=0 的后端不应收到流量

---

## Extended Tests (1 test)

### 5. GRPCRouteNamedRule
- **Features**: Gateway, GRPCRoute, GRPCRouteNamedRouteRule
- **Manifest**: `grpcroute-named-rule.yaml`
- **描述**: gRPC 命名路由规则
- **测试逻辑**:
  1. 创建 GRPCRoute，rules 带有 `name` 字段
  2. 验证路由正常工作
  3. 验证 status 中反映了命名规则信息
