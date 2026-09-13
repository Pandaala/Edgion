# GRPCRoute 总览

GRPCRoute 用于基于 gRPC 服务名、方法名、主机名等条件进行路由，适用于微服务内部 gRPC 流量治理。

## 基本结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: grpc-route
  namespace: production
spec:
  parentRefs:             # 绑定到哪个 Gateway
    - name: main-gateway
      sectionName: grpc
  hostnames:              # 匹配的域名
    - "grpc.example.com"
  rules:                  # 路由规则列表
    - matches:            # 匹配条件（可选）
        - method:
            service: myapp.UserService
            method: GetUser
      backendRefs:        # 后端服务
        - name: user-service
          port: 50051
```

关键字段说明：

| 字段 | 说明 |
|------|------|
| `parentRefs` | 将路由绑定到一个或多个 Gateway 监听器 |
| `hostnames` | 匹配请求的 `:authority` 头（精确或通配符） |
| `rules` | 路由规则列表，每条包含 matches、filters、backendRefs |

## 核心概念

### ParentRefs

指定此路由绑定到哪个 Gateway 的哪个监听器。GRPCRoute 通常绑定到 HTTPS 或 gRPC 协议的监听器：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | Gateway 名称 |
| `namespace` | String | 否 | Gateway 命名空间（默认同路由） |
| `sectionName` | String | 否 | 监听器名称 |

### Rules

每条 Rule 是一个独立的路由决策单元，包含匹配条件、过滤器和后端引用。匹配到的规则按[优先级规则](matches/overview.md#优先级)选择；声明顺序仅用于同一路由内最后的平局判定。

### Matches

GRPCRoute 支持按 gRPC 方法进行匹配：

- **method.service**：匹配 gRPC 服务名（如 `myapp.UserService`）
- **method.method**：匹配 gRPC 方法名（如 `GetUser`）
- **method.type**：匹配类型，`Exact`（默认）或 `RegularExpression`
- **headers**：匹配 gRPC 元数据头

同一 Match 内的条件为 AND 关系，多个 Match 之间为 OR 关系。详见 [匹配规则](./matches/overview.md)。

### Filters / Plugins

在请求到达后端之前执行处理逻辑：

- **Gateway API 标准 Filter**：`RequestHeaderModifier`、`ResponseHeaderModifier`、`RequestMirror` 等
- **Edgion 扩展 Plugin**：通过 `ExtensionRef` 引用 `EdgionPlugins` CRD

详见 [过滤器](./filters/overview.md)。

### BackendRefs

定义 gRPC 流量转发的后端服务列表：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | Service 名称 |
| `port` | Integer | 是 | gRPC 端口号 |
| `weight` | Integer | 否 | 权重（默认 `1`），用于加权分流 |

详见 [后端配置](./backends/overview.md)。

## 完整示例

以下示例展示了一个包含 Gateway 和 GRPCRoute 的完整配置，覆盖 TLS 终止、方法匹配、多后端分流等常用功能：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: grpc-https
      protocol: HTTPS
      port: 443
      hostname: grpc.example.com
      tls:
        mode: Terminate
        certificateRefs:
          - group: ""
            kind: Secret
            name: grpc-cert
---
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: app-grpc-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: grpc-https
  hostnames:
    - grpc.example.com
  rules:
    # 规则 1：精确匹配 UserService.GetUser
    - matches:
        - method:
            service: myapp.UserService
            method: GetUser
      backendRefs:
        - name: user-service
          port: 50051

    # 规则 2：匹配 OrderService 全部方法，灰度分流
    - matches:
        - method:
            service: myapp.OrderService
      backendRefs:
        - name: order-service-stable
          port: 50051
          weight: 90
        - name: order-service-canary
          port: 50051
          weight: 10

    # 规则 3：默认后端
    - backendRefs:
        - name: default-grpc-service
          port: 50051
```

## 子文档导航

| 主题 | 文档 | 说明 |
|------|------|------|
| 匹配规则 | [matches/](./matches/overview.md) | gRPC 服务名、方法名、元数据头匹配 |
| 过滤器 | [filters/](./filters/overview.md) | Gateway API 标准 Filter 与 Edgion 扩展 Plugin |
| 后端配置 | [backends/](./backends/overview.md) | 多后端分流、权重配置 |
