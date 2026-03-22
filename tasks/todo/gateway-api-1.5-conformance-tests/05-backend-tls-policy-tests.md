# BackendTLSPolicy Conformance Tests (6 tests)

## Core Tests (5 tests)

### 1. BackendTLSPolicy
- **Features**: Gateway, HTTPRoute, BackendTLSPolicy
- **Manifest**: `backend-tls-policy.yaml`
- **描述**: 基本 BackendTLSPolicy 功能
- **测试逻辑**:
  1. 创建后端 Service，启用 TLS
  2. 创建 BackendTLSPolicy，引用 CA 证书
  3. 配置 `targetRefs` 指向后端 Service
  4. 配置 `validation.caCertificateRefs` 引用 CA ConfigMap
  5. 配置 `validation.hostname` 为后端证书的 CN/SAN
  6. 创建 HTTPRoute 路由到该后端
  7. 验证网关使用 TLS 连接后端 (后端 TLS)
  8. 验证 CA 证书验证通过
  9. 请求成功到达后端

### 2. BackendTLSPolicyConflictResolution
- **Features**: Gateway, HTTPRoute, BackendTLSPolicy
- **Manifest**: `backend-tls-policy-conflict-resolution.yaml`
- **描述**: 多个 BackendTLSPolicy 冲突时的解决
- **测试逻辑**:
  1. 创建同一后端 Service 的两个 BackendTLSPolicy
  2. 验证冲突处理:
     - 按名称字典序，较早的 policy 优先
     - 或两者都标记为 Conflicted
  3. 验证 status.conditions 反映冲突状态
  4. 只有一个 policy 生效

### 3. BackendTLSPolicyInvalidCACertificateRef
- **Features**: Gateway, HTTPRoute, BackendTLSPolicy
- **Manifest**: `backend-tls-policy-invalid-ca-certificate-ref.yaml`
- **描述**: 无效的 CA 证书引用
- **测试逻辑**:
  1. BackendTLSPolicy 引用不存在的 CA ConfigMap/Secret
  2. 验证 Accepted 条件为 False
  3. reason: `InvalidCertificateRef`
  4. 后端连接应失败或路由不生效

### 4. BackendTLSPolicyInvalidKind
- **Features**: Gateway, HTTPRoute, BackendTLSPolicy
- **Manifest**: `backend-tls-policy-invalid-kind.yaml`
- **描述**: targetRef 引用无效的 Kind
- **测试逻辑**:
  1. BackendTLSPolicy 的 targetRef 指定不支持的 Kind (非 Service)
  2. 验证 Accepted 条件为 False
  3. reason: `InvalidTargetRef`

### 5. BackendTLSPolicyObservedGenerationBump
- **Features**: Gateway, HTTPRoute, BackendTLSPolicy
- **Manifest**: `backend-tls-policy-observed-generation-bump.yaml`
- **描述**: BackendTLSPolicy 更新后 observedGeneration 递增
- **测试逻辑**:
  1. 创建 BackendTLSPolicy
  2. 验证 observedGeneration == generation
  3. 更新 policy
  4. 验证 observedGeneration 递增

---

## Extended Tests (1 test)

### 6. BackendTLSPolicySANValidation
- **Features**: Gateway, HTTPRoute, BackendTLSPolicy, BackendTLSPolicySANValidation
- **Manifest**: `backend-tls-policy-san.yaml`
- **描述**: Subject Alternative Name (SAN) 验证
- **测试逻辑**:
  1. BackendTLSPolicy 配置 `validation.subjectAltNames`
  2. 测试:
     - SAN 匹配后端证书 → 连接成功
     - SAN 不匹配 → 连接失败
  3. 支持的 SAN 类型:
     - Hostname (DNS SAN)
     - URI SAN
  4. 验证 hostname 字段和 subjectAltNames 字段的交互
