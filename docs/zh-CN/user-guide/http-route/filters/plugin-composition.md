# 插件组合与引用

> **🔌 Edgion 扩展**
>
> EdgionPlugins 通过 HTTPRoute / GRPCRoute 的 `ExtensionRef` filter（Gateway API 标准）被路由引用，支持多路由共享同一份插件链配置。

## Overview

HTTPRoute 和 GRPCRoute 在 `rules[].filters` 里通过 `ExtensionRef` 引用一个 `EdgionPlugins` 资源，路由请求命中时执行该资源里声明的插件链。HTTPBackendRef 的 `filters` 同样支持 `ExtensionRef`。

> **Note:** `EdgionPlugins.spec.*Plugins` 内部的 `type: ExtensionRef`（即"插件引用插件"嵌套形态）已在 release `<version>` 移除。复用请使用下列两种推荐模式。详见 `docs/zh-CN/migrations/<version>-extension-ref.md`。

## 基本用法

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: shared-bundle
  namespace: default
spec:
  requestPlugins:
    - type: ipRestriction
      config:
        defaultAction: allow
        rules:
          - action: deny
            cidr: "10.0.0.0/8"
    - type: basicAuth
      config:
        users:
          - username: admin
            password: secret
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle
```

## 推荐复用模式

### 1. Aggregate 模式（推荐主用）

把多个插件合并到一个 `EdgionPlugins` 资源，路由只需一次 `ExtensionRef`。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: extended-security
  namespace: default
spec:
  requestPlugins:
    - type: ipRestriction
      config:
        defaultAction: allow
        rules:
          - action: deny
            cidr: "10.0.0.0/8"
    - type: basicAuth
      config:
        users:
          - username: admin
            password: secret
```

### 2. Side-by-side 模式

在 `HTTPRoute.rules[].filters` 里并列写多个 `ExtensionRef`，分别指向不同的 `EdgionPlugins`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle-1
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle-2
```

### GRPCRoute

GRPCRoute filter list 上的 `ExtensionRef` 用法与 HTTPRoute 一致：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: my-grpc-route
  namespace: default
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: shared-bundle
```

## 行为

- **Per-stage fanout**：一次 `ExtensionRef` 声明 → 自动 fanout 到 request / upstream_response_filter / upstream_response_body_filter 三个 stage，每个 stage 自动从目标 `EdgionPlugins` 拉同 stage 子集。
- **动态解析**：插件引用在运行时按 `namespace/name` 从 `PluginStore` 解析，支持热更新。
- **错误处理**：目标 `EdgionPlugins` 不存在时，按各 stage 默认策略处理；错误信息计入 access log。

## 设计原则

对齐 Envoy filter chain 模型：filter chain 是扁平 list，不是 graph。复用通过"多 Route 引用同一资源"实现（对标 Envoy ECDS），不通过"插件引用插件"。

## Best Practices

- 优先用 aggregate 模式：把语义相关的插件合并到同一个 `EdgionPlugins`。
- 用 side-by-side 模式把彼此正交的能力（如 auth 与 rate-limit）分别维护成独立 `EdgionPlugins`，按需在 route filter list 里组合。
- 用描述性的资源名（如 `shared-auth-stack`、`canary-rate-limit`）便于跨团队复用。
