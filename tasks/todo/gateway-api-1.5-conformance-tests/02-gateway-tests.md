# Gateway / GatewayClass Conformance Tests (21 tests)

## GatewayClass Tests (1 test)

### 1. GatewayClassObservedGenerationBump
- **Features**: Gateway
- **Manifest**: `gatewayclass-observed-generation-bump.yaml`
- **描述**: GatewayClass 更新后 observedGeneration 递增
- **测试逻辑**:
  1. 创建 GatewayClass
  2. 验证 `status.conditions[].observedGeneration` == `metadata.generation`
  3. 更新 GatewayClass (如修改 description)
  4. 验证 observedGeneration 跟随 generation 递增
  5. 验证 Accepted 条件为 True

---

## Gateway Core Tests (8 tests)

### 2. GatewayObservedGenerationBump
- **Features**: Gateway
- **Manifest**: `gateway-observed-generation-bump.yaml`
- **描述**: Gateway 更新后 observedGeneration 递增
- **测试逻辑**:
  1. 创建 Gateway
  2. 验证 status.conditions 和 listeners[].conditions 中 observedGeneration 正确
  3. 更新 Gateway (如修改 listener)
  4. 验证所有 conditions 的 observedGeneration 跟随递增

### 3. GatewayInvalidRouteKind
- **Features**: Gateway
- **Manifest**: `gateway-invalid-route-kind.yaml`
- **描述**: Listener 配置了无效的路由 Kind
- **测试逻辑**:
  1. Gateway listener 的 `allowedRoutes.kinds` 指定不存在的 Kind
  2. 验证 listener 状态:
     - `ResolvedRefs` 条件为 False
     - reason: `InvalidRouteKinds`
  3. 验证 `supportedKinds` 只包含有效的 Kind

### 4. GatewayInvalidTLSConfiguration
- **Features**: Gateway
- **Manifest**: `gateway-invalid-tls-configuration.yaml`
- **描述**: 无效的 TLS 配置
- **测试逻辑**:
  1. Gateway listener 配置 HTTPS 但 TLS 配置无效 (如缺少证书引用、模式错误)
  2. 验证 listener 状态:
     - `Accepted` 或 `Programmed` 条件为 False
     - reason 指示 TLS 配置错误

### 5. GatewayModifyListeners
- **Features**: Gateway
- **Manifest**: `gateway-modify-listeners.yaml`
- **描述**: 动态修改 Gateway listener
- **测试逻辑**:
  1. 创建 Gateway 带一个 listener
  2. 验证 listener 正常
  3. 添加新 listener
  4. 验证新 listener 被接受和 Programmed
  5. 删除一个 listener
  6. 验证剩余 listener 仍正常
  7. 修改 listener 端口
  8. 验证修改生效

### 6. GatewayWithAttachedRoutes
- **Features**: Gateway, HTTPRoute
- **Manifest**: `gateway-with-attached-routes.yaml`
- **描述**: Gateway listener 的 attachedRoutes 计数
- **测试逻辑**:
  1. 创建 Gateway
  2. 创建多个 HTTPRoute 绑定到该 Gateway
  3. 验证 `status.listeners[].attachedRoutes` 正确反映绑定的路由数量
  4. 删除/添加路由后数量更新

### 7. GatewaySecretReferenceGrantAllInNamespace
- **Features**: Gateway, ReferenceGrant
- **Manifest**: `gateway-secret-reference-grant-all-in-namespace.yaml`
- **描述**: ReferenceGrant 允许 Gateway 引用其他命名空间的所有 Secret
- **测试逻辑**:
  1. Gateway 的 HTTPS listener 引用另一个命名空间的 TLS Secret
  2. 在 Secret 命名空间创建 ReferenceGrant，允许所有来自 Gateway 命名空间的 Gateway 引用 Secret
  3. 验证 listener 的 ResolvedRefs 条件为 True
  4. HTTPS listener 正常工作

### 8. GatewaySecretReferenceGrantSpecific
- **Features**: Gateway, ReferenceGrant
- **Manifest**: `gateway-secret-reference-grant-specific.yaml`
- **描述**: ReferenceGrant 允许引用特定 Secret
- **测试逻辑**:
  1. 与上面类似，但 ReferenceGrant 只允许引用特定名称的 Secret
  2. 验证允许的 Secret 可正常引用
  3. 不在允许列表中的 Secret 不能引用

### 9. GatewaySecretInvalidReferenceGrant
- **Features**: Gateway, ReferenceGrant
- **Manifest**: `gateway-secret-invalid-reference-grant.yaml`
- **描述**: ReferenceGrant 不匹配时 Secret 引用失败
- **测试逻辑**:
  1. Gateway HTTPS listener 引用另一个命名空间的 Secret
  2. ReferenceGrant 存在但不允许该引用 (如 from 不匹配)
  3. 验证 ResolvedRefs 条件为 False
  4. reason: `RefNotPermitted`

---

## Gateway Extended Tests - Secret (1 test)

### 10. GatewaySecretMissingReferenceGrant
- **Features**: Gateway, ReferenceGrant
- **Manifest**: `gateway-secret-missing-reference-grant.yaml`
- **描述**: 缺少 ReferenceGrant 时跨命名空间 Secret 引用
- **测试逻辑**:
  1. Gateway HTTPS listener 引用其他命名空间的 Secret
  2. 完全没有 ReferenceGrant
  3. 验证 ResolvedRefs 条件为 False
  4. reason: `RefNotPermitted`

---

## Gateway Extended Tests - 地址 (2 tests)

### 11. GatewayStaticAddresses
- **Features**: Gateway, GatewayStaticAddresses
- **Manifest**: `gateway-static-addresses.yaml`
- **描述**: Gateway 静态地址分配
- **测试逻辑**:
  1. 创建 Gateway，`spec.addresses` 配置静态 IP 地址
  2. 验证 `status.addresses` 包含请求的地址
  3. 验证地址类型 (IPAddress/Hostname) 正确

### 12. GatewayOptionalAddressValue
- **Features**: Gateway, GatewayAddressEmpty
- **Manifest**: `gateway-optional-address-value.yaml`
- **描述**: Gateway 地址值可选 (空地址)
- **测试逻辑**:
  1. 创建 Gateway，不指定 `spec.addresses`
  2. 验证控制器自动分配地址
  3. `status.addresses` 不为空
  4. 地址类型为实现默认值

---

## Gateway Extended Tests - 基础设施 (1 test)

### 13. GatewayInfrastructure
- **Features**: Gateway, GatewayInfrastructurePropagation
- **Manifest**: `gateway-infrastructure.yaml`
- **描述**: Gateway 基础设施配置传播
- **测试逻辑**:
  1. 创建 Gateway，`spec.infrastructure` 配置 labels 和 annotations
  2. 验证底层基础设施资源 (Pod, Service) 继承这些 labels 和 annotations
  3. 更新 infrastructure 配置
  4. 验证传播更新

---

## Gateway Extended Tests - TLS 后端 (2 tests)

### 14. GatewayInvalidTLSBackendConfiguration
- **Features**: Gateway, GatewayBackendClientCertificate, HTTPRoute, BackendTLSPolicy
- **Manifest**: `gateway-invalid-tls-backend-configuration.yaml`
- **描述**: 无效的 TLS 后端配置
- **测试逻辑**:
  1. Gateway 配置了 `backendTLS` 但引用的客户端证书 Secret 无效或不存在
  2. 验证相关条件反映配置错误
  3. BackendTLSPolicy 引用与 Gateway backendTLS 冲突时的行为

### 15. GatewayBackendClientCertificateFeature
- **Features**: Gateway, GatewayBackendClientCertificate, HTTPRoute, BackendTLSPolicy
- **Manifest**: `gateway-backend-client-certificate.yaml`
- **描述**: Gateway 后端客户端证书 (mTLS)
- **测试逻辑**:
  1. Gateway 配置 `spec.backendTLS.clientCertificateRef` 引用客户端证书 Secret
  2. BackendTLSPolicy 启用后端 TLS
  3. 后端要求客户端证书 (mTLS)
  4. 验证网关使用配置的客户端证书连接后端
  5. 请求成功路由

---

## Gateway Extended Tests - 前端客户端证书验证 (4 tests)

### 16. GatewayFrontendClientCertificateValidation
- **Features**: Gateway, HTTPRoute, GatewayFrontendClientCertificateValidation
- **Manifest**: `gateway-frontend-client-certificate-validation.yaml`
- **描述**: 前端客户端证书验证 (入口 mTLS)
- **测试逻辑**:
  1. Gateway HTTPS listener 配置 `tls.frontendValidation`
  2. 引用 CA 证书 ConfigMap/Secret
  3. 客户端发送带有效证书的请求 → 成功
  4. 客户端发送无证书的请求 → 失败 (TLS 握手错误)
  5. 客户端发送无效证书的请求 → 失败

### 17. GatewayFrontendClientCertificateValidationInsecureFallback
- **Features**: Gateway, HTTPRoute, GatewayFrontendClientCertificateValidation, GatewayFrontendClientCertificateValidationInsecureFallback
- **Manifest**: `gateway-frontend-client-certificate-validation-insecure-fallback.yaml`
- **描述**: 前端证书验证的不安全回退
- **测试逻辑**:
  1. HTTPS listener 配置前端证书验证，但允许不安全回退
  2. 有效证书 → 正常响应
  3. 无证书 → 仍然允许连接 (回退到无验证)
  4. 无效证书 → 仍然拒绝

### 18. GatewayFrontendInvalidDefaultClientCertificateValidation
- **Features**: Gateway, HTTPRoute, GatewayFrontendClientCertificateValidation
- **Manifest**: `gateway-frontend-invalid-default-client-certificate-validation.yaml`
- **描述**: 无效的前端证书验证配置
- **测试逻辑**:
  1. HTTPS listener 配置了无效的前端验证 (如引用不存在的 CA)
  2. 验证 listener 状态反映配置错误
  3. `ResolvedRefs` 或 `Accepted` 条件为 False

### 19. GatewayInvalidFrontendClientCertificateValidation
- **Features**: Gateway, HTTPRoute, ReferenceGrant, GatewayFrontendClientCertificateValidation
- **Manifest**: `gateway-invalid-frontend-client-certificate-validation.yaml`
- **描述**: 跨命名空间前端证书引用无效
- **测试逻辑**:
  1. Gateway 的前端验证引用其他命名空间的 CA 证书
  2. 没有对应的 ReferenceGrant
  3. 验证 ResolvedRefs 条件为 False

---

## Gateway Extended Tests - HTTP Listener 隔离 (1 test)

### 20. GatewayHTTPListenerIsolation
- **Features**: Gateway, GatewayHTTPListenerIsolation, HTTPRoute
- **Manifest**: `gateway-http-listener-isolation.yaml`
- **描述**: 同一 Gateway 上多个 HTTP listener 之间的流量隔离
- **测试逻辑**:
  1. Gateway 有多个 HTTP listener，每个绑定不同 hostname
  2. 不同 HTTPRoute 通过 sectionName 绑定到不同 listener
  3. 发送请求到 listener A 的 hostname → 只路由到 A 绑定的路由
  4. 不同 listener 的路由互不干扰

*注: 此测试同时出现在 Gateway 和 HTTPRoute 测试列表中*

---

## Gateway Extended Tests - Listener 隔离 (1 test, counted in HTTPRoute)

### 21. HTTPRouteHTTPSListenerDetectMisdirectedRequests
*已在 HTTPRoute 测试中详述 (测试 #51)*
- **Features**: Gateway, GatewayHTTPSListenerDetectMisdirectedRequests, HTTPRoute
- SNI 与 Host header 不匹配时返回 421
