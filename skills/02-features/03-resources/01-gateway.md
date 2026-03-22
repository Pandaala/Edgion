---
name: gateway-resource
description: Gateway 资源完整 Schema：Listener、协议、TLS、AllowedRoutes、Status。
---

# Gateway 资源

> API: `gateway.networking.k8s.io/v1` | Scope: Namespaced
> Gateway API v1.4.0 Core 资源

Gateway 是 Edgion 的核心入口资源，定义网关实例的 Listener（监听端口、协议、主机名、TLS 配置），是所有路由资源的挂载点。

## 完整 Schema

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: my-gateway
  namespace: default
  annotations:
    # Edgion 扩展注解
    edgion.io/enable-http2: "true"              # 启用 HTTP/2（默认 false）
    edgion.io/backend-protocol: "HTTP"          # 后端协议：HTTP | HTTPS | H2 | H2C
    edgion.io/http-to-https-redirect: "true"    # HTTP→HTTPS 自动重定向
    edgion.io/https-redirect-port: "443"        # HTTPS 重定向端口
    edgion.io/edgion-stream-plugins: "ns/name"  # Gateway 级别 StreamPlugins 绑定
spec:
  gatewayClassName: edgion                       # 必填：关联的 GatewayClass 名称

  listeners:
    - name: http                                 # 必填：Listener 唯一名称
      hostname: "*.example.com"                  # 可选：匹配的主机名（支持通配符）
      port: 80                                   # 必填：监听端口
      protocol: HTTP                             # 必填：协议类型
      allowedRoutes:                             # 可选：路由挂载规则
        namespaces:
          from: Same                             # Same | All | Selector
          # selector:                            # from=Selector 时必填
          #   matchLabels: {}
        kinds:                                   # 可选：允许的路由类型
          - group: gateway.networking.k8s.io
            kind: HTTPRoute

    - name: https
      hostname: "*.example.com"
      port: 443
      protocol: HTTPS
      tls:                                       # HTTPS/TLS 协议必填
        mode: Terminate                          # Terminate | Passthrough
        certificateRefs:                         # mode=Terminate 时的证书引用
          - name: my-cert-secret                 # Secret 名称
            namespace: default                   # 可选：跨命名空间需 ReferenceGrant
            kind: Secret                         # 默认 Secret
            group: ""                            # 默认 core group
        options:                                 # Edgion 扩展选项
          edgion.io/cert-provider: "edgion-tls"  # 证书来源："secret"(默认) | "edgion-tls"

    - name: tcp
      port: 9000
      protocol: TCP

    - name: tls-passthrough
      hostname: "secure.example.com"
      port: 8443
      protocol: TLS
      tls:
        mode: Passthrough                        # TLS 透传（不终止）

    - name: udp
      port: 5353
      protocol: UDP

  addresses:                                     # 可选：请求分配的地址
    - type: IPAddress                            # IPAddress | Hostname
      value: "10.0.0.1"
```

## Listener 配置详解

### 协议支持

| Protocol | 适用路由 | TLS 要求 | 说明 |
|----------|---------|---------|------|
| `HTTP` | HTTPRoute, GRPCRoute | — | 纯 HTTP 流量 |
| `HTTPS` | HTTPRoute, GRPCRoute | `tls` 必填 | TLS 终止后的 HTTP 流量 |
| `TLS` | TLSRoute | `tls` 必填 | TLS 终止或透传 |
| `TCP` | TCPRoute | — | 原始 TCP 流量 |
| `UDP` | UDPRoute | — | UDP 流量 |

### TLS 配置 Schema

```yaml
tls:
  mode: String           # "Terminate" | "Passthrough"
  certificateRefs:       # mode=Terminate 时的证书列表
    - name: String       # Secret 名称（必填）
      namespace: String? # 命名空间（跨命名空间需 ReferenceGrant）
      kind: String?      # 默认 "Secret"
      group: String?     # 默认 ""（core group）
  options:               # 实现特定选项（JSON）
    edgion.io/cert-provider: String   # "secret" | "edgion-tls"
  # secrets: [...]       # 运行时填充（Controller 解析后注入，用户不填写）
```

### AllowedRoutes Schema

```yaml
allowedRoutes:
  namespaces:
    from: String         # "Same" | "All" | "Selector"
    selector:            # from=Selector 时必填
      matchLabels: {}    # K8s label selector
  kinds:                 # 允许挂载的路由类型列表
    - group: String?     # 默认 "gateway.networking.k8s.io"
      kind: String       # HTTPRoute | GRPCRoute | TCPRoute | TLSRoute | UDPRoute
```

**from 值**:
| 值 | 说明 |
|----|------|
| `Same` | 仅允许与 Gateway 同命名空间的路由（默认） |
| `All` | 允许所有命名空间的路由 |
| `Selector` | 按 label selector 过滤命名空间 |

## Status Schema

```yaml
status:
  addresses:                              # 分配的地址
    - type: IPAddress
      value: "10.0.0.1"
  conditions:                             # Gateway 级别条件
    - type: Accepted                      # 资源已接受
      status: "True"
      reason: Accepted
    - type: ListenersNotValid             # 有端口冲突时为 True
      status: "False"
      reason: Valid
  listeners:                              # 每个 Listener 的状态
    - name: http
      supportedKinds:                     # 该 Listener 支持的路由类型
        - group: gateway.networking.k8s.io
          kind: HTTPRoute
      attachedRoutes: 3                   # 已挂载路由数
      conditions:
        - type: Accepted
          status: "True"
        - type: Conflicted                # 端口冲突
          status: "False"
        - type: ResolvedRefs              # 证书引用已解析
          status: "True"
```

## 端口冲突检测

同一 `(port, protocol, hostname)` 组合在多个 Gateway 间冲突时，后创建的 Gateway 的 Listener 会被标记为 `Conflicted=True`。冲突 Gateway 被删除后，自动恢复。

## Edgion 扩展注解

| 注解 | 值 | 默认 | 说明 |
|------|---|------|------|
| `edgion.io/enable-http2` | `"true"\|"false"` | `"false"` | Listener 启用 HTTP/2 |
| `edgion.io/backend-protocol` | `HTTP\|HTTPS\|H2\|H2C` | `HTTP` | 上游后端协议 |
| `edgion.io/http-to-https-redirect` | `"true"\|"false"` | `"false"` | HTTP→HTTPS 自动重定向 |
| `edgion.io/https-redirect-port` | `port` | `"443"` | HTTPS 重定向目标端口 |
| `edgion.io/edgion-stream-plugins` | `ns/name` | — | Gateway 级别 StreamPlugins |
