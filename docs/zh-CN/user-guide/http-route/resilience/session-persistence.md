# 会话保持

配置会话保持（Session Affinity）确保同一客户端的请求路由到同一后端。

## 工作原理

1. **首次请求**：客户端未携带会话标识 → 网关使用正常负载均衡选择后端 → 响应中注入会话标识（Cookie 或 Header）
2. **后续请求**：客户端携带会话标识 → 网关基于一致性哈希将请求路由到同一后端
3. **后端不可用**：默认 fallback 到其他健康后端并更新会话标识；strict 模式下返回错误

会话标识是后端地址的 CRC32 哈希（如 `a1b2c3d4`），不暴露后端拓扑信息。

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
| idleTimeout | duration | 否 | — | 空闲超时（当前版本不支持，预留给 Redis 模式） |
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

Cookie 属性：`Path=/; Max-Age=86400; HttpOnly; SameSite=Lax`

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
| **Session Persistence** | **中** | **基于 cookie/header 的一致性哈希** |
| DynamicInternalUpstream | 低 | 指定 BackendRef，会话保持仍在其内生效 |
| 默认 Round-Robin | 最低 | 无会话保持时的默认行为 |

## 示例

### 示例 1：Cookie 会话保持（带 Fallback）

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: session-sticky
spec:
  parentRefs:
    - name: my-gateway
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
    - name: my-gateway
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

- **多实例一致性**：基于一致性哈希算法，多 Gateway 实例间只要后端列表一致，同一 token 会路由到同一后端。后端扩缩容时哈希环变化可能导致少量 session 漂移。
- **idleTimeout**：当前版本不支持服务端 idle timeout（纯客户端 Cookie 方案无法实现），预留给后续 Redis 模式。
- **GRPCRoute**：共享同一套会话保持逻辑，配置方式相同。

## 相关文档

- [超时配置](./timeouts.md)
- [重试策略](./retry.md)
- [负载均衡算法](../lb-algorithms.md)
