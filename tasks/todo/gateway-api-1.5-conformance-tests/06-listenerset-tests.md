# ListenerSet Conformance Tests (10 tests)

> **v1.5 新增**: ListenerSet 是 Gateway API v1.5.0 的新特性，允许将额外的 listener 附加到现有 Gateway 上。

## 命名空间控制 Tests (3 tests)

### 1. ListenerSetAllowedNamespaceNone
- **Features**: Gateway, ListenerSet
- **Manifest**: `listenerset-allowed-namespace-none.yaml`
- **描述**: ListenerSet 不允许来自任何命名空间
- **测试逻辑**:
  1. Gateway 配置 `spec.allowedListeners.namespaces.from: None`
  2. 创建 ListenerSet 引用该 Gateway
  3. 验证 ListenerSet 不被接受 (Accepted=False)
  4. 没有额外的 listener 被添加到 Gateway

### 2. ListenerSetAllowedNamespaceSame
- **Features**: Gateway, ListenerSet
- **Manifest**: `listenerset-allowed-namespace-same.yaml`
- **描述**: ListenerSet 只允许同命名空间
- **测试逻辑**:
  1. Gateway 配置 `spec.allowedListeners.namespaces.from: Same`
  2. 同命名空间的 ListenerSet → accepted
  3. 不同命名空间的 ListenerSet → not accepted
  4. 验证 listener 只从同命名空间的 ListenerSet 添加

### 3. ListenerSetAllowedNamespaceSelector
- **Features**: Gateway, ListenerSet
- **Manifest**: `listenerset-allowed-namespace-selector.yaml`
- **描述**: ListenerSet 通过 label selector 控制命名空间
- **测试逻辑**:
  1. Gateway 配置 namespace selector (如 `env: production`)
  2. 带匹配 label 命名空间中的 ListenerSet → accepted
  3. 不带匹配 label 的命名空间 → not accepted

---

## 路由控制 Tests (2 tests)

### 4. ListenerSetAllowedRoutesNamespaces
- **Features**: Gateway, ListenerSet, HTTPRoute, ReferenceGrant
- **Manifest**: `listenerset-allowed-routes-namespaces.yaml`
- **描述**: ListenerSet 添加的 listener 的路由命名空间控制
- **测试逻辑**:
  1. ListenerSet 创建 listener，配置 `allowedRoutes.namespaces`
  2. HTTPRoute 从允许的命名空间绑定 → accepted
  3. HTTPRoute 从不允许的命名空间 → not accepted
  4. 验证 ReferenceGrant 在跨命名空间场景中的作用

### 5. ListenerSetAllowedRoutesSupportedKinds
- **Features**: Gateway, ListenerSet, TLSRoute
- **Manifest**: `listenerset-allowed-routes-supported-kinds.yaml`
- **描述**: ListenerSet listener 的 supportedKinds 过滤
- **测试逻辑**:
  1. ListenerSet 创建 listener，配置 `allowedRoutes.kinds`
  2. 验证 status 中 `supportedKinds` 正确反映配置
  3. 不在 allowedRoutes.kinds 中的路由类型不被接受

---

## 安全和冲突 Tests (3 tests)

### 6. ListenerSetDefaultNotAllowed
- **Features**: Gateway, ListenerSet
- **Manifest**: `listenerset-default-not-allowed.yaml`
- **描述**: 默认情况下不允许 ListenerSet
- **测试逻辑**:
  1. Gateway 没有显式配置 `allowedListeners`
  2. 创建 ListenerSet
  3. 验证默认行为: ListenerSet 不被接受
  4. 必须显式允许才能使用 ListenerSet

### 7. ListenerSetHostnameConflict
- **Features**: Gateway, ListenerSet
- **Manifest**: `listenerset-hostname-conflict.yaml`
- **描述**: ListenerSet listener 与 Gateway listener hostname 冲突
- **测试逻辑**:
  1. Gateway 有 listener 绑定 `example.com`
  2. ListenerSet 添加 listener 也绑定 `example.com` 同端口同协议
  3. 验证冲突处理:
     - Gateway 自身的 listener 优先
     - ListenerSet listener 标记为 Conflicted
  4. 验证 listener conditions 反映冲突状态

### 8. ListenerSetProtocolConflict
- **Features**: Gateway, ListenerSet
- **Manifest**: `listenerset-protocol-conflict.yaml`
- **描述**: ListenerSet listener 与 Gateway listener 协议冲突
- **测试逻辑**:
  1. Gateway 有 HTTP listener 在某端口
  2. ListenerSet 尝试在同端口添加 HTTPS listener
  3. 验证协议冲突被检测
  4. 冲突的 listener 不被 Programmed

---

## 功能 Tests (2 tests)

### 9. ListenerSetHTTPRouting
- **Features**: Gateway, ListenerSet, HTTPRoute
- **Manifest**: `listenerset-http-routing.yaml`
- **描述**: 通过 ListenerSet 添加的 listener 进行 HTTP 路由
- **测试逻辑**:
  1. Gateway 有基础 listener
  2. ListenerSet 添加额外的 HTTP listener (新端口或新 hostname)
  3. HTTPRoute 绑定到 ListenerSet 添加的 listener
  4. 发送 HTTP 请求到新 listener
  5. 验证请求正确路由到后端
  6. 验证 `attachedRoutes` 正确

### 10. ListenerSetReferenceGrant
- **Features**: Gateway, ListenerSet, ReferenceGrant
- **Manifest**: `listenerset-reference-grant.yaml`
- **描述**: 跨命名空间 ListenerSet 的 ReferenceGrant 控制
- **测试逻辑**:
  1. ListenerSet 在不同命名空间引用 Gateway
  2. 需要 ReferenceGrant 允许该引用
  3. 有 ReferenceGrant → ListenerSet accepted
  4. 无 ReferenceGrant → ListenerSet not accepted
  5. 验证 ReferenceGrant 的 from/to 配置正确
