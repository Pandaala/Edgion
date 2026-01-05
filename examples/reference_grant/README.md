# ReferenceGrant 权限验证示例

## 功能说明

ReferenceGrant 是 Kubernetes Gateway API 的一个资源，用于显式授权跨 namespace 的资源引用。

例如：
- 当 `ns-app` 中的 HTTPRoute 需要引用 `ns-shared` 中的 Service 时
- 当 `ns-frontend` 中的 Gateway 需要引用 `ns-certs` 中的 Secret 时

Edgion 支持在启动时验证这些跨 namespace 引用是否被 ReferenceGrant 允许。

## 启用验证

在 `EdgionGatewayConfig` 中添加：

```yaml
spec:
  enableReferenceGrantValidation: true  # 默认为 false
```

## 示例场景

### 场景 1: HTTPRoute 跨 namespace 引用 Service

1. **创建 ReferenceGrant**（在目标 namespace）

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-app-to-shared-services
  namespace: ns-shared  # 目标 namespace（Service 所在）
spec:
  from:
  - group: gateway.networking.k8s.io
    kind: HTTPRoute
    namespace: ns-app  # 源 namespace（Route 所在）
  to:
  - group: ""
    kind: Service
```

2. **创建 HTTPRoute**（在源 namespace）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: ns-app
spec:
  parentRefs:
  - name: my-gateway
  rules:
  - backendRefs:
    - name: my-service
      namespace: ns-shared  # 跨 namespace 引用
      port: 80
```

### 场景 2: Gateway 跨 namespace 引用 Secret

1. **创建 ReferenceGrant**（在 Secret 所在的 namespace）

```yaml
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-gateway-to-tls-secrets
  namespace: ns-certs
spec:
  from:
  - group: gateway.networking.k8s.io
    kind: Gateway
    namespace: ns-gateway
  to:
  - group: ""
    kind: Secret
```

2. **创建 Gateway**

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: ns-gateway
spec:
  gatewayClassName: edgion
  listeners:
  - name: https
    protocol: HTTPS
    port: 443
    tls:
      mode: Terminate
      certificateRefs:
      - name: my-tls-cert
        namespace: ns-certs  # 跨 namespace 引用
```

## 验证行为

### 验证开启时 (`enableReferenceGrantValidation: true`)

- 启动时，网关会检查所有跨 namespace 引用
- 如果没有对应的 ReferenceGrant，会记录 **warn** 日志
- 资源仍然会被加载（不会阻止启动）
- ReferenceGrant 更新时，会自动触发相关资源的重新验证

**日志示例**:
```
WARN Cross-namespace reference validation failed: HTTPRoute in namespace 'ns-app' cannot reference Service/my-service in namespace 'ns-shared' (no ReferenceGrant)
```

### 验证关闭时 (`enableReferenceGrantValidation: false`，默认)

- 不进行任何跨 namespace 引用检查
- 不加载 ReferenceGrant 资源（节省内存）
- 适用于单 namespace 部署或不需要严格权限控制的场景

## 最佳实践

1. **生产环境建议开启验证**
   - 提供更好的安全性和可见性
   - 明确跨 namespace 的依赖关系

2. **开发/测试环境可以关闭**
   - 简化配置
   - 快速迭代

3. **使用最小权限原则**
   - ReferenceGrant 应该尽可能具体（指定 name 而不是允许所有）
   - 定期审查不再需要的 ReferenceGrant

## 常见问题

**Q: 验证失败会导致启动失败吗？**
A: 不会。验证失败只会记录 warn 日志，资源仍然会被加载。这样设计是为了避免配置错误导致整个网关不可用。

**Q: 我的配置都在同一个 namespace，需要开启验证吗？**
A: 不需要。如果没有跨 namespace 引用，建议保持默认关闭状态。

**Q: 动态更新 ReferenceGrant 会立即生效吗？**
A: 是的。当 ReferenceGrant 被添加、修改或删除时，网关会自动重新验证相关的资源。

