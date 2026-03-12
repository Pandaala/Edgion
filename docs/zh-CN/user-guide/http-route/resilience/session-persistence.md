# 会话保持

配置会话保持（Session Affinity）确保同一客户端的请求路由到同一后端。

## 配置

```yaml
rules:
  - sessionPersistence:
      sessionName: my-session
      type: Cookie
      cookieConfig:
        lifetimeType: Permanent
        path: /
    backendRefs:
      - name: app-service
        port: 8080
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| sessionName | string | | | 会话名称 |
| type | string | | Cookie | 会话类型 |
| absoluteTimeout | duration | | | 绝对超时 |
| idleTimeout | duration | | | 空闲超时 |
| cookieConfig | object | | | Cookie 配置 |

### cookieConfig

| 字段 | 类型 | 说明 |
|------|------|------|
| lifetimeType | string | Session 或 Permanent |
| path | string | Cookie 路径 |

## 会话类型

### Cookie

基于 Cookie 的会话保持：

```yaml
sessionPersistence:
  type: Cookie
  sessionName: SERVERID
  cookieConfig:
    lifetimeType: Permanent
```

### Header

基于请求头的会话保持：

```yaml
sessionPersistence:
  type: Header
  sessionName: X-Session-ID
```

## 示例

### 示例 1: Cookie 会话保持

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
        cookieConfig:
          lifetimeType: Session
      backendRefs:
        - name: stateful-app
          port: 8080
          weight: 50
        - name: stateful-app-2
          port: 8080
          weight: 50
```

## 相关文档

- [超时配置](./timeouts.md)
- [重试策略](./retry.md)
