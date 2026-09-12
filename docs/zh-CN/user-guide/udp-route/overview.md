# UDPRoute 总览

UDPRoute 用于 UDP 流量转发，适合 DNS、日志采集、游戏通信等场景。

## 基本结构

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-route
  namespace: default
spec:
  parentRefs:             # 绑定到哪个 Gateway
    - name: my-gateway
      sectionName: udp
  rules:                  # 路由规则列表
    - backendRefs:        # 后端服务
        - name: my-udp-service
          port: 53
```

关键字段说明：

| 字段 | 说明 |
|------|------|
| `parentRefs` | 将路由绑定到一个或多个 Gateway 的 UDP 监听器 |
| `rules` | 路由规则列表，每条包含 backendRefs |

> UDPRoute 工作在四层，不支持 Matches 和 Filters，路由决策仅依赖监听器绑定关系。

## 核心概念

### ParentRefs

指定此路由绑定到哪个 Gateway 的哪个 UDP 监听器：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | Gateway 名称 |
| `namespace` | String | 否 | Gateway 命名空间（默认同路由） |
| `sectionName` | String | 否 | 监听器名称 |

每个 UDP 监听器通常只绑定一个 UDPRoute，因为没有七层匹配条件来区分流量。

### Rules

UDPRoute 的 Rule 只包含 backendRefs，没有 matches 和 filters 字段。每条 Rule 直接将流量转发到指定后端。

### BackendRefs

定义 UDP 流量转发的后端服务列表：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | Service 名称 |
| `port` | Integer | 是 | 端口号 |
| `weight` | Integer | 否 | 权重（默认 `1`），用于加权分流 |

## 完整示例

以下示例展示了一个完整的 Gateway + UDPRoute 配置，将 DNS 和 syslog 流量通过网关转发到后端：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: edge-gw
  namespace: gateway-system
spec:
  gatewayClassName: edgion
  listeners:
    - name: udp-dns
      protocol: UDP
      port: 53
    - name: udp-syslog
      protocol: UDP
      port: 514
---
# DNS 路由
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: dns-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-dns
  rules:
    - backendRefs:
        - name: dns-service
          port: 53
---
# Syslog 路由
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: syslog-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: udp-syslog
  rules:
    - backendRefs:
        - name: syslog-service
          port: 514
```

## 注意事项

1. UDP 无连接，重试与会话语义与 TCP/HTTP 不同。
2. 需要结合后端应用层做可靠性补偿。
3. 一旦客户端会话建立，其上游就会在会话生命周期内固定：路由或后端的变更只对新会话生效。现有会话在 60 秒无活动后超时，之后下一个数据包会重新执行路由匹配。
4. Gateway 关闭时，UDP 在 Pingora 的关闭阶段仍继续服务；Pingora 停止后，UDP 停止收发，并在共享的五秒辅助服务收尾窗口内清理任务和会话。超时会强制取消任务并以非零状态退出；取消完成仍依赖运行时调度，五秒不是严格的进程退出上限。关闭可能丢弃在途数据报，应用应自行处理重试。

## 子文档导航

| 主题 | 文档 | 说明 |
|------|------|------|
| 后端配置 | [backends/](./backends/overview.md) | 后端服务引用与权重配置 |
| Stream Plugins | [stream-plugins.md](../tcp-route/stream-plugins.md) | TCP/UDP 流量的 Edgion 扩展插件 |
