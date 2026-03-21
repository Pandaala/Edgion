---
name: grpcroute-features
description: GRPCRoute 完整 Schema：gRPC service/method 匹配、过滤器、gRPC-Web 支持。
---

# GRPCRoute 资源

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.4.0 Core 资源

GRPCRoute 定义 gRPC 协议的路由规则，支持按 service/method 匹配和 gRPC-Web。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: my-grpc-route
  namespace: default
  annotations:
    edgion.io/max-retries: "3"
spec:
  parentRefs:
    - name: my-gateway
      sectionName: https

  hostnames:
    - "grpc.example.com"

  rules:
    - matches:
        - method:
            type: Exact                            # Exact | RegularExpression
            service: "mypackage.MyService"         # gRPC service 名称
            method: "GetItem"                      # gRPC method 名称
          headers:
            - type: Exact
              name: x-custom-header
              value: "value"

      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-backend-version
                value: "v2"

        - type: ResponseHeaderModifier
          responseHeaderModifier:
            add:
              - name: x-trace-id
                value: "{{generated}}"

        - type: RequestMirror
          requestMirror:
            backendRef:
              name: mirror-service
              port: 50051
            fraction:
              numerator: 5
              denominator: 100

        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: grpc-auth

      backendRefs:
        - name: grpc-service
          port: 50051
          weight: 100

      timeouts:
        request: "30s"
        backendRequest: "10s"

      retry:
        attempts: 3
        backoff: "500ms"
        codes:                                     # gRPC 状态码
          - 14                                     # UNAVAILABLE

      sessionPersistence:
        type: Cookie
        sessionName: "GRPC_SESSION"
```

## 匹配条件 Schema

### GRPCMethodMatch

| 字段 | 类型 | 说明 |
|------|------|------|
| `type` | `String` | `Exact` / `RegularExpression` |
| `service` | `String?` | gRPC service 全限定名（如 `mypackage.MyService`） |
| `method` | `String?` | gRPC method 名称 |

匹配逻辑：
- `service` + `method` 都指定 → 精确匹配单个方法
- 只指定 `service` → 匹配该 service 的所有方法
- 都不指定 → 匹配所有 gRPC 请求

### GRPCHeaderMatch

与 HTTPRoute 的 headers 匹配相同。

## 过滤器类型

| 类型 | 说明 |
|------|------|
| `RequestHeaderModifier` | 修改请求头 |
| `ResponseHeaderModifier` | 修改响应头 |
| `RequestMirror` | 请求镜像 |
| `ExtensionRef` | 引用 EdgionPlugins |

**注意**：GRPCRoute 不支持 `RequestRedirect` 和 `URLRewrite`。

## gRPC-Web 支持

Edgion 自动检测 gRPC-Web 请求（通过 `content-type` 头），无需额外配置。gRPC-Web 请求按 GRPCRoute 规则路由。

## 与 HTTPRoute 的区别

| 维度 | HTTPRoute | GRPCRoute |
|------|----------|-----------|
| 匹配维度 | path + headers + query + method | service + method + headers |
| 过滤器 | 6 种（含 Redirect、URLRewrite） | 4 种（无 Redirect、URLRewrite） |
| 内部实现 | 独立匹配引擎 | 共享 route_utils，gRPC 特有 MatchInfo |
| gRPC-Web | 不支持 | 自动检测 |
