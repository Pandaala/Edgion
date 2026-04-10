# DebugAccessLogToHeader 插件

> **🔌 Edgion 扩展**
>  
> DebugAccessLogToHeader 是 `EdgionPlugins` CRD 提供的调试插件，将当前请求的完整 access log 序列化为 JSON 字符串注入响应头，不属于标准 Gateway API。

> ⚠️ **仅限开发/测试环境**
>  
> 此插件会将 IP 地址、路由信息、请求元数据等敏感数据暴露在 HTTP 响应头中，**严禁在生产环境使用**。

## 什么是 DebugAccessLogToHeader？

通常排查 access log 内容需要登录服务器或访问日志存储系统。DebugAccessLogToHeader 让你无需访问日志文件——它在请求处理完成后，将完整的 access log（与写入日志文件的内容相同）序列化为 JSON，注入到响应头 `X-Debug-Access-Log` 中。你可以用 `curl -v` 或浏览器开发者工具直接查看 access log 内容。

## 快速开始

### 最简单的配置

此插件无任何配置参数，直接启用即可。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: debug-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "dev.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-access-log
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-access-log
  namespace: default
spec:
  requestPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

启用后，每个响应都会包含类似如下的响应头：

```
X-Debug-Access-Log: {"remote_addr":"192.168.1.1","method":"GET","uri":"/api/users","status":200,"latency_ms":12,...}
```

---

## 配置参数

此插件无配置参数。

---

## 常见配置场景

### 1. 全路由启用（开发环境）

在开发网关的所有路由上全局启用调试头，方便随时查看任意请求的 access log。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: dev-all-debug
  namespace: dev
spec:
  parentRefs:
    - name: dev-gateway
  hostnames:
    - "dev.example.com"
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-log-header
      backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-log-header
  namespace: dev
spec:
  requestPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

### 2. 仅对特定路径生效

只在 `/api/v2/` 路径下注入调试头，不影响其他路由。适合在测试环境中针对特定接口调试。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: partial-debug
  namespace: test
spec:
  parentRefs:
    - name: test-gateway
  hostnames:
    - "test.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/v2
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-v2-only
      backendRefs:
        - name: backend-service
          port: 8080
    - backendRefs:
        - name: backend-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-v2-only
  namespace: test
spec:
  requestPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

### 3. 与其他插件组合使用

将调试头与认证插件组合，调试已认证请求的 access log（如确认 JWT 解析是否正确注入到 access log 中）。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-with-auth
  namespace: dev
spec:
  requestPlugins:
    - type: JwtAuth
      config:
        secret: "dev-secret"
    - type: DebugAccessLogToHeader
      config: {}
```

插件按顺序执行，`DebugAccessLogToHeader` 捕获的是认证完成后的完整 access log（含 JWT 解析结果）。

---

## 常见问题

### Q1：`X-Debug-Access-Log` 头中能看到哪些信息？

**A**：与 access log 文件中记录的内容相同，通常包括客户端 IP、请求方法、URI、响应状态码、延迟、上游地址、插件处理结果等。具体字段由 access log 配置决定，详见 [Access Log 文档](../../../../ops-guide/observability/access-log.md)。

### Q2：响应头内容过长被截断了怎么办？

**A**：HTTP 响应头有长度限制（取决于客户端和代理），过长的 access log 可能被截断。如果遇到此问题，建议直接查看日志文件，或在 access log 配置中减少字段数量。

### Q3：能在生产环境临时开启用来排查问题吗？

**A**：不推荐。即使是短暂开启，也可能将用户 IP、认证信息等敏感数据暴露给请求方，违反安全最佳实践。生产环境排查问题请使用 [Access Log](../../../../ops-guide/observability/access-log.md) 或分布式追踪系统。

---

## 安全最佳实践

### 推荐做法

- 仅在开发、测试、预发布环境的命名空间中创建此插件
- 通过 Kubernetes RBAC 限制 `EdgionPlugins` 资源的创建权限
- 使用命名空间隔离，防止测试插件被误用于生产路由

### 避免的做法

- 不要在生产环境创建此插件，即使是临时的
- 不要将包含此插件的 `EdgionPlugins` 资源部署到生产命名空间
- 不要将 `X-Debug-Access-Log` 头的内容记录到客户端可访问的日志系统

---

## 相关文档

- [AllEndpointStatus 插件](all-endpoint-status.md) — 批量探查后端 endpoint
- [Access Log 配置](../../../../ops-guide/observability/access-log.md) — access log 字段与格式说明
- [Mock 插件](mock.md) — 静态响应返回（调试用）
