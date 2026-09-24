# 会话保持

配置会话保持（Session Affinity）确保同一客户端的请求路由到同一后端。

## 工作原理

1. **首次请求**：客户端未携带会话标识 → 网关使用正常（加权）负载均衡选择后端 → 响应中注入会话标识（Cookie 或 Header）
2. **后续请求**：客户端携带会话标识 → 网关在加权选择**之前**，在规则的全部后端范围内解析该标识，将请求精确路由到签发它的那个后端
3. **后端不可用**：默认 fallback 到其他健康后端并更新会话标识；strict 模式下返回错误

会话标识是后端地址的 SHA-256 截断哈希（16 位小写十六进制，如 `a1b2c3d4e5f60718`），不暴露后端拓扑信息。

## 流量分割（多加权 BackendRef）

依照 Gateway API（GEP-1619），**权重只管理新会话**。会话一旦建立，将在规则的所有
backendRefs 范围内持续路由到最初的 Service 和端点：

- 会话标识在加权选择之前、按声明顺序在匹配规则的每个 backendRef 内解析，
  因此 70/30 分流不会让已建立的会话在 Service 之间弹跳。
- 修改绑定 backendRef 的权重——**包括改为 0**——只要其端点仍在服务，
  已建立的会话就不会迁移。
- 删除绑定的端点（或 backendRef）触发正常 fallback：请求由加权选择接管，
  响应携带刷新后的会话标识。
- 会话解析的作用域是匹配到的路由规则：两条使用默认会话名、后端集合不相交的
  规则不会共享会话；引用同一 Service 的规则会解析到同一 pod（与 Envoy
  Gateway 行为一致）。

```yaml
rules:
  - sessionPersistence:
      type: Cookie
      sessionName: canary-session
      absoluteTimeout: "1h"
      cookieConfig:
        lifetimeType: Permanent
    backendRefs:
      - name: app-v1
        port: 8080
        weight: 70        # 新会话约 70%
      - name: app-v2
        port: 8080
        weight: 30        # 新会话约 30%；已建立的 v2 会话固定在 v2
```

## 配置

```yaml
rules:
  - sessionPersistence:
      sessionName: my-session              # Cookie/Header 名称
      type: Cookie                         # Cookie | Header
      absoluteTimeout: "1h"                # 绝对超时（映射为 Cookie Max-Age）
      cookieConfig:
        lifetimeType: Permanent            # Permanent | Session
    backendRefs:
      - name: app-service
        port: 8080
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| sessionName | string | 否 | `edgion-session-affinity` | 会话 token 名称（Cookie 名或 Header 名） |
| type | string | 否 | Cookie | 会话类型：`Cookie` 或 `Header` |
| absoluteTimeout | duration | 否 | — | 绝对超时（Permanent Cookie 模式下映射为 `Max-Age`） |
| cookieConfig | object | 否 | — | Cookie 特有配置 |
| strict | bool | 否 | false | Edgion 扩展：strict 模式（见下方说明） |

### cookieConfig

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| lifetimeType | string | Session | `Session`（浏览器关闭失效）或 `Permanent`（需配合 absoluteTimeout 设置 Max-Age） |

## 会话类型

### Cookie（默认）

基于 Cookie 的会话保持。网关在首次响应中注入 `Set-Cookie`，客户端后续请求自动携带。

```yaml
sessionPersistence:
  type: Cookie
  sessionName: BACKEND_ID
  absoluteTimeout: "24h"
  cookieConfig:
    lifetimeType: Permanent
```

Cookie 属性：`Path=/; Max-Age=86400; HttpOnly; SameSite=Lax`。当请求通过 TLS 到达时，`Secure` 属性会自动添加。`Max-Age` 仅在 `lifetimeType: Permanent` 且设置了 `absoluteTimeout` 时才会输出。

### Header

基于自定义 Header 的会话保持。网关在首次响应中添加 Header，客户端需在后续请求中携带该 Header。

```yaml
sessionPersistence:
  type: Header
  sessionName: X-Session-ID
```

## Fallback 与 Strict 模式

### 默认行为（Fallback）

当会话绑定的后端不可用时（健康检查失败、后端下线等），网关自动 fallback 到其他健康后端，并在响应中更新会话标识。客户端后续请求将路由到新后端。

这与 Nginx、Envoy、HAProxy 的默认行为一致。

### Strict 模式

设置 `strict: true` 后，会话绑定的后端不可用时直接返回错误（503），不做 fallback。适用于对会话一致性有严格要求的场景。

```yaml
sessionPersistence:
  type: Cookie
  sessionName: BACKEND_ID
  strict: true                  # 不允许 fallback
  cookieConfig:
    lifetimeType: Permanent
```

> 类似 Envoy stateful session filter 的 `strict` 字段和 HAProxy 的 `force-persist`。

## 与插件的优先级关系

| 插件 | 优先级 | 与会话保持的关系 |
|------|--------|-----------------|
| DirectEndpoint | 最高 | 直接指定后端 IP:port，跳过会话保持 |
| DynamicExternalUpstream | 高 | 外部域名路由，跳过会话保持 |
| DynamicInternalUpstream | 高 | 显式的单请求 BackendRef 覆盖：本次请求优先于粘性会话；会话绑定保持不变（不重写标识），下一个未覆盖的请求正常解析 |
| **Session Persistence** | **中** | **基于 cookie/header 标识的精确后端亲和** |
| 加权 Round-Robin | 最低 | 管理无标识的会话（初始分配） |

## 示例

### 示例 1：Cookie 会话保持（带 Fallback）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: session-sticky
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - sessionPersistence:
        type: Cookie
        sessionName: BACKEND_ID
        absoluteTimeout: "1h"
        cookieConfig:
          lifetimeType: Permanent
      backendRefs:
        - name: stateful-app
          port: 8080
```

### 示例 2：Header 会话保持（Strict 模式）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: session-strict
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - sessionPersistence:
        type: Header
        sessionName: X-Session-ID
        strict: true
      backendRefs:
        - name: stateful-app
          port: 8080
```

## 限制

Gateway API v1.6.2 已从 HTTPRoute 和 GRPCRoute 的 `sessionPersistence` 中移除 `idleTimeout`。请勿配置该字段；API Server 会按 CRD 将其剪除。

- **多实例一致性**：会话标识派生是确定性的（后端地址的无密钥哈希），多 Gateway 实例间只要后端列表一致，同一 token 就路由到同一后端，无需服务端会话存储。删除绑定端点会使其会话失效（fallback + 标识刷新），其他端点的会话不受影响。
- **GRPCRoute**：原生支持，配置方式与 HTTPRoute 相同，并有基于后端身份的集成测试验证。gRPC 特有语义：`strict: true` 时，粘性后端不可用将以 `grpc-status: 14`（UNAVAILABLE）失败关闭，而非 HTTP 503。原生 gRPC 客户端（tonic、grpc-go）需要在应用层自行回传会话 Cookie；浏览器 grpc-web 客户端仅在同站（same-site）且启用凭据（credentials）时自动携带（Cookie 带有 `SameSite=Lax`），跨站 grpc-web 部署建议使用 `type: Header`。会话保持依赖 EndpointSlice 类后端（ClusterIP 类后端会静默绕过，与 HTTPRoute 行为一致）。

## 相关文档

- [超时配置](./timeouts.md)
- [重试策略](./retry.md)
- [负载均衡算法](../lb-algorithms.md)
