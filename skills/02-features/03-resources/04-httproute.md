---
name: httproute-features
description: HTTPRoute 完整 Schema 与功能：匹配条件、过滤器、后端引用、超时/重试、会话保持。
---

# HTTPRoute 资源

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.4.0 Core 资源

HTTPRoute 是 Edgion 中最复杂的路由资源，定义 HTTP/HTTPS 层的路由规则。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
  annotations:
    edgion.io/max-retries: "3"                    # Edgion 扩展：最大重试次数
spec:
  parentRefs:                                      # 挂载到 Gateway（见 00-common-concepts.md）
    - name: my-gateway
      sectionName: https

  hostnames:                                       # 匹配的主机名列表
    - "api.example.com"
    - "*.example.com"                              # 支持通配符

  rules:
    - name: "api-v1"                               # 规则名称（可选，用于可观测性）

      matches:                                     # 匹配条件（OR 关系）
        - path:
            type: PathPrefix                       # Exact | PathPrefix | RegularExpression
            value: "/api/v1"
          headers:
            - type: Exact                          # Exact | RegularExpression
              name: X-Version
              value: "v1"
          queryParams:
            - type: Exact                          # Exact | RegularExpression
              name: debug
              value: "true"
          method: GET                              # HTTP 方法

      filters:                                     # 过滤器链
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: X-Gateway
                value: edgion
            add:
              - name: X-Request-ID
                value: "{{generated}}"
            remove:
              - X-Internal

        - type: ResponseHeaderModifier
          responseHeaderModifier:
            set:
              - name: X-Powered-By
                value: Edgion

        - type: RequestRedirect
          requestRedirect:
            scheme: https
            hostname: new.example.com
            port: 443
            statusCode: 301                        # 301 | 302
            path:
              type: ReplaceFullPath                # ReplaceFullPath | ReplacePrefixMatch
              replaceFullPath: /new-path

        - type: URLRewrite
          urlRewrite:
            hostname: internal.example.com
            path:
              type: ReplacePrefixMatch
              replacePrefixMatch: /v2

        - type: RequestMirror
          requestMirror:
            backendRef:
              name: mirror-service
              port: 8080
            fraction:
              numerator: 10
              denominator: 100                     # 10% 镜像
            connectTimeoutMs: 1000                 # 连接超时（ms）
            writeTimeoutMs: 1000                   # 写入超时（ms）
            maxBufferedChunks: 5                   # 最大缓冲 body chunk 数
            mirrorLog: true                        # 镜像是否单独记录 access log
            maxConcurrent: 1024                    # 最大并发镜像任务
            channelFullTimeoutMs: 0                # channel 满时等待（ms）

        - type: ExtensionRef                       # 引用 EdgionPlugins
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-plugins
          extensionRefMaxDepth: 5                  # 嵌套 ExtensionRef 最大深度

      backendRefs:                                 # 后端引用
        - name: api-service
          port: 8080
          weight: 80
        - name: api-service-canary
          port: 8080
          weight: 20
          filters:                                 # backendRef 级别过滤器
            - type: RequestHeaderModifier
              requestHeaderModifier:
                set:
                  - name: X-Canary
                    value: "true"

      timeouts:                                    # Gateway API v1.4 标准超时
        request: "30s"                             # 端到端请求超时（含重试）
        backendRequest: "10s"                      # 单次后端请求超时

      retry:                                       # Gateway API v1.4 标准重试
        attempts: 3                                # 最大重试次数
        backoff: "1s"                              # 重试间隔
        codes:                                     # 触发重试的 HTTP 状态码
          - 502
          - 503
          - 504

      sessionPersistence:                          # 会话保持
        sessionName: "EDGION_SESSION"              # 会话 token 名称
        absoluteTimeout: "1h"                      # 绝对超时
        idleTimeout: "30m"                         # 空闲超时
        type: Cookie                               # Cookie | Header
        cookieConfig:
          lifetimeType: Permanent                  # Permanent | Session
```

## 匹配条件 Schema

### HTTPRouteMatch

多个 match 之间是 **OR** 关系；单个 match 内的 path/headers/queryParams/method 是 **AND** 关系。

#### path

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | `Exact` / `PathPrefix` / `RegularExpression` |
| `value` | `String` | 匹配值（PathPrefix 默认 `/`） |

#### headers

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | `Exact` / `RegularExpression` |
| `name` | `String` | Header 名称（大小写不敏感） |
| `value` | `String` | 匹配值 |

#### queryParams

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | `Exact` / `RegularExpression` |
| `name` | `String` | 查询参数名称 |
| `value` | `String` | 匹配值 |

#### method

`String`: `GET` / `POST` / `PUT` / `DELETE` / `PATCH` / `HEAD` / `OPTIONS` / `CONNECT`

## 过滤器类型

| 类型 | 说明 | 阶段 |
|------|------|------|
| `RequestHeaderModifier` | 修改请求头（set/add/remove） | 请求阶段 |
| `ResponseHeaderModifier` | 修改响应头（set/add/remove） | 响应阶段 |
| `RequestRedirect` | HTTP 重定向（301/302） | 请求阶段（终止） |
| `URLRewrite` | URL 重写（hostname/path） | 请求阶段 |
| `RequestMirror` | 请求镜像到备份后端 | 请求阶段（非阻塞） |
| `ExtensionRef` | 引用 EdgionPlugins 扩展插件 | 取决于插件 |

### 过滤器互斥约束

`URLRewrite` 和 `RequestRedirect` **互斥**，不能在同一规则中同时出现。冲突检查覆盖三个层级：

1. **规则级**：`rule.filters` 中同时包含 URLRewrite 和 RequestRedirect
2. **BackendRef 级**：`backendRef.filters` 中同时包含两者
3. **跨级别**：`rule.filters` 包含其中一个，`backendRef.filters` 包含另一个

违反约束的规则将被标记为 `PartiallyInvalid`，该规则不会生效，但 HTTPRoute 中其他有效规则仍正常工作。

### ExtensionRef 嵌套深度限制

`ExtensionRef` 引用的 `EdgionPlugins` 内部可以继续包含 `ExtensionRef`，形成嵌套引用链。为防止递归引用导致的性能问题，Edgion 限制嵌套解析的最大深度：

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `extensionRefMaxDepth` | `u32` | `5` | 嵌套 ExtensionRef 解析的最大层数 |

超过深度限制的引用将被忽略，不会触发错误。大多数场景下默认值 5 已足够，仅在插件链特别复杂时才需要调整。

## 超时与重试

### timeouts (Gateway API v1.4)

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `request` | `Duration` | EdgionGatewayConfig 全局值 | 端到端请求超时（**含重试**） |
| `backendRequest` | `Duration` | EdgionGatewayConfig 全局值 | 单次后端请求超时（不得超过 `request`） |

> **约束**：`backendRequest` 不能超过 `request`。如果配置的 `backendRequest` 大于 `request`，将被自动 clamp 为 `request` 的值，确保单次后端请求超时不会超过端到端限制。

### retry (Gateway API v1.4)

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `attempts` | `i32` | — | 最大重试次数 |
| `backoff` | `Duration` | — | 重试最小间隔 |
| `codes` | `Vec<i32>` | — | 触发重试的 HTTP 状态码 |

### Edgion 扩展注解

| 注解 | 类型 | 说明 |
|------|------|------|
| `edgion.io/max-retries` | `u32` | 最大重试次数（与 retry.attempts 互补） |

## 会话保持 Schema

```yaml
sessionPersistence:
  sessionName: String?               # 会话 token 名称（默认 edgion-session-affinity）
  absoluteTimeout: Duration?         # 绝对超时（Permanent Cookie 映射为 Max-Age）
  idleTimeout: Duration?             # 空闲超时（当前版本不支持，预留 Redis 模式）
  type: Cookie | Header              # 保持类型（默认 Cookie）
  cookieConfig:
    lifetimeType: Permanent | Session # Cookie 生命周期
  strict: bool?                      # Edgion 扩展：strict 模式（默认 false）
```

### 运行时行为

- **Token 生成**：后端 `ip:port` 的 CRC32 哈希（8 位 hex），不暴露后端拓扑
- **Cookie 模式**：首次响应注入 `Set-Cookie: <name>=<token>; Path=/; HttpOnly; SameSite=Lax`（Permanent 加 `Max-Age`）
- **Header 模式**：首次响应注入 `<name>: <token>`，客户端需在后续请求中携带
- **后端选择**：Token 作为一致性哈希 key，确保同一 token 路由到同一后端
- **Fallback**（默认）：绑定后端不可用时 fallback 到正常 LB，更新 token
- **Strict 模式**：`strict: true` 时不 fallback，返回 503

### 与插件的优先级

DirectEndpoint / ExternalJump > Session Persistence > InternalJump > Round-Robin

## 流量分割

通过 `backendRefs` 的 `weight` 字段实现：

```yaml
backendRefs:
  - name: v1-service
    port: 8080
    weight: 90    # 90% 流量
  - name: v2-service
    port: 8080
    weight: 10    # 10% 流量（金丝雀）
```
