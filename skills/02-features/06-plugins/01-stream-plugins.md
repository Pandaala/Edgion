---
name: stream-plugins-features
description: EdgionStreamPlugins CRD Schema：TCP/TLS 层两阶段插件。
---

# EdgionStreamPlugins — Stream 插件

> API: `edgion.io/v1` | Scope: Namespaced

EdgionStreamPlugins 定义 TCP/TLS 层的插件，在 Pingora 的 ConnectionFilter 阶段执行，工作在 HTTP 解析之前。

## 完整 Schema

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: my-stream-plugins
  namespace: default
spec:
  # Stage 1: ConnectionFilter（pre-TLS，仅 IP 信息可用）
  plugins:
    - enable: true
      plugin:
        type: IpRestriction
        config:
          allowlist:
            - "10.0.0.0/8"
            - "192.168.1.0/24"
          denylist:
            - "10.0.0.100"

  # Stage 2: TlsRoute（post-TLS handshake，SNI/证书信息可用）
  tlsRoutePlugins:
    - enable: true
      plugin:
        type: IpRestriction
        config:
          allowlist:
            - "172.16.0.0/12"
```

## 两阶段模型

| 阶段 | 触发时机 | 可用信息 | 字段 |
|------|---------|---------|------|
| Stage 1: ConnectionFilter | TCP 连接建立后、TLS 握手前 | 源 IP、目标端口 | `spec.plugins` |
| Stage 2: TlsRoute | TLS 握手完成后 | SNI、客户端证书、IP | `spec.tlsRoutePlugins` |

## 绑定方式

通过注解绑定到不同级别：

```yaml
# Gateway 级别 — 所有连接
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "default/my-stream-plugins"

# TCPRoute 级别
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "default/tcp-plugins"

# TLSRoute 级别
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "default/tls-plugins"
```

## 当前支持的 Stream 插件

| 类型 | 阶段 | 说明 |
|------|------|------|
| `IpRestriction` | Stage 1 + Stage 2 | IP 黑白名单（CIDR 支持） |

## 与 HTTP 插件的区别

| 维度 | HTTP 插件 (EdgionPlugins) | Stream 插件 (EdgionStreamPlugins) |
|------|--------------------------|----------------------------------|
| 层级 | L7 HTTP 层 | L4 TCP/TLS 层 |
| 执行位置 | ProxyHttp 回调 | ConnectionFilter |
| 可用上下文 | 完整 HTTP 请求 | 仅连接信息（IP、SNI） |
| 失败行为 | 返回 HTTP 错误 | 关闭连接 |
| 绑定方式 | ExtensionRef Filter | 注解 |
| 热更新来源 | ClientCache → ConfHandler | 注解解析 → Store 查找 |

## Status Schema

```yaml
status:
  conditions:
    - type: Accepted
      status: "True"
      reason: Valid
```
