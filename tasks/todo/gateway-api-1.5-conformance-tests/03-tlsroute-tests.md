# TLSRoute Conformance Tests (13 tests)

## Core Tests - Passthrough 模式 (8 tests)

### 1. TLSRouteSimpleSameNamespace
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-simple-same-namespace.yaml`
- **描述**: 同命名空间内简单 TLS passthrough 路由
- **测试逻辑**:
  1. Gateway 配置 TLS listener，mode=Passthrough
  2. 创建 TLSRoute 在同命名空间，绑定到该 Gateway
  3. 验证 TLSRoute 被 accepted
  4. 发送 TLS 请求 (SNI 匹配)
  5. 验证请求直接透传到后端 (不做 TLS 终止)
  6. 后端收到完整的 TLS 连接

### 2. TLSRouteHostnameIntersection
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-hostname-intersection.yaml`
- **描述**: TLSRoute 和 Gateway listener hostname 的交集
- **测试逻辑**:
  1. Gateway listener 有 hostname 限制
  2. TLSRoute 定义 hostname
  3. 只有交集 hostname 生效
  4. 测试:
     - 匹配交集的 SNI → 路由成功
     - 只在 TLSRoute 中但不在 listener 中的 SNI → 不路由
     - 通配符交集行为

### 3. TLSRouteInvalidBackendRefNonexistent
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-invalid-backendref-nonexistent.yaml`
- **描述**: TLSRoute 引用不存在的后端
- **测试逻辑**:
  1. TLSRoute 的 backendRef 引用不存在的 Service
  2. 验证 ResolvedRefs 条件为 False
  3. reason: `BackendNotFound`

### 4. TLSRouteInvalidBackendRefUnknownKind
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-invalid-backendref-unknown-kind.yaml`
- **描述**: TLSRoute 引用未知类型的后端
- **测试逻辑**:
  1. TLSRoute 的 backendRef 指定不支持的 Kind
  2. 验证 ResolvedRefs 条件为 False
  3. reason: `InvalidKind`

### 5. TLSRouteInvalidNoMatchingListenerHostname
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-invalid-no-matching-listener-hostname.yaml`
- **描述**: TLSRoute hostname 与 listener 不匹配
- **测试逻辑**:
  1. TLSRoute 的 hostname 与 Gateway 任何 listener 的 hostname 都不交集
  2. 验证 Accepted 条件为 False
  3. reason: `NoMatchingListenerHostname`

### 6. TLSRouteInvalidNoMatchingListener
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-invalid-no-matching-listener.yaml`
- **描述**: TLSRoute 无法匹配任何 listener
- **测试逻辑**:
  1. TLSRoute 的 parentRef 指向不存在或不兼容的 listener
  2. 验证 Accepted 条件为 False
  3. 可能因为协议不匹配 (如引用 HTTP listener)

### 7. TLSRouteInvalidReferenceGrant
- **Features**: Gateway, TLSRoute, ReferenceGrant
- **Manifest**: `tlsroute-invalid-reference-grant.yaml`
- **描述**: TLSRoute 跨命名空间引用无 ReferenceGrant
- **测试逻辑**:
  1. TLSRoute 引用另一个命名空间的 backend Service
  2. 没有对应的 ReferenceGrant
  3. 验证 ResolvedRefs 条件为 False
  4. reason: `RefNotPermitted`

### 8. TLSRouteListenerPassthroughSupportedKinds
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-listener-passthrough-supported-kinds.yaml`
- **描述**: Passthrough listener 的 supportedKinds
- **测试逻辑**:
  1. Gateway listener 配置 TLS mode=Passthrough
  2. 验证 listener status 中 `supportedKinds` 包含 TLSRoute
  3. 验证 `supportedKinds` 不包含 HTTPRoute (因为 passthrough 模式)

---

## Extended Tests - Terminate 模式 (4 tests)

### 9. TLSRouteListenerTerminateNotSupported
- **Features**: Gateway, TLSRoute
- **Manifest**: `tlsroute-listener-terminate-not-supported.yaml`
- **描述**: 不支持 Terminate 模式的实现对 TLSRoute 的处理
- **测试逻辑**:
  1. Gateway listener 配置 TLS mode=Terminate
  2. TLSRoute 绑定到该 listener
  3. 如果实现不支持 TLSRouteModeTerminate:
     - listener supportedKinds 不包含 TLSRoute (对于 Terminate 模式)
     - 或 TLSRoute 不被 accepted

### 10. TLSRouteListenerTerminateSupportedKinds
- **Features**: Gateway, TLSRoute, TLSRouteModeTerminate
- **Manifest**: `tlsroute-listener-terminate-supported-kinds.yaml`
- **描述**: Terminate listener 的 supportedKinds 包含 TLSRoute
- **测试逻辑**:
  1. Gateway listener 配置 TLS mode=Terminate
  2. 验证 listener status 中 `supportedKinds` 包含 TLSRoute
  3. 验证 TLSRoute 可以绑定到 Terminate 模式的 listener

### 11. TLSRouteTerminateSimpleSameNamespace
- **Features**: Gateway, TLSRoute, TLSRouteModeTerminate
- **Manifest**: `tlsroute-terminate-simple-same-namespace.yaml`
- **描述**: TLS Terminate 模式基本路由
- **测试逻辑**:
  1. Gateway listener 配置 TLS mode=Terminate，引用 TLS 证书
  2. TLSRoute 绑定到该 listener
  3. 客户端发送 TLS 请求
  4. 网关终止 TLS 连接
  5. 使用 SNI 匹配 TLSRoute
  6. 以明文 TCP 连接后端
  7. 验证后端收到明文流量

### 12. TLSRouteListenerMixedTerminationNotSupported
- **Features**: Gateway, TLSRoute, TLSRouteModeTerminate
- **Manifest**: `tlsroute-listener-mixed-termination-not-supported.yaml`
- **描述**: 不支持混合终止模式的处理
- **测试逻辑**:
  1. 同一 Gateway 上有 Passthrough 和 Terminate 两种 listener
  2. 如果实现不支持 TLSRouteModeMixed:
     - 验证 Mixed 模式下 TLSRoute 不被 accepted
     - 或相关 listener 状态正确反映不支持

---

## Extended Tests - Mixed 模式 (1 test)

### 13. TLSRouteMixedTerminationSameNamespace
- **Features**: Gateway, TLSRoute, TLSRouteModeTerminate, TLSRouteModeMixed
- **Manifest**: `tlsroute-mixed-termination-same-namespace.yaml`
- **描述**: 混合终止模式 - 同一 TLSRoute 同时支持 Passthrough 和 Terminate
- **测试逻辑**:
  1. Gateway 有两个 listener:
     - 一个 TLS mode=Passthrough (如 `*.passthrough.example.com`)
     - 一个 TLS mode=Terminate (如 `*.terminate.example.com`)
  2. 单个 TLSRoute 绑定到两个 listener
  3. 测试 Passthrough 路径:
     - SNI 匹配 passthrough hostname → TLS 直接透传
  4. 测试 Terminate 路径:
     - SNI 匹配 terminate hostname → 网关终止 TLS，明文转发
  5. 验证两种模式在同一 TLSRoute 下同时工作
  6. **v1.5 新增**: 这是 experimental 功能
