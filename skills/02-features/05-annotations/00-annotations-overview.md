---
name: annotations-overview
description: Edgion 注解总览：按资源类型分类的所有 edgion.io/* 注解快速查找。
---

# 注解总览

## 前缀规范

- 所有 Edgion 注解使用 `edgion.io/` 前缀（**不是** `edgion.com/`）
- Stream 插件注解键是 `edgion-stream-plugins`（**不是** `stream-plugins`）

## 按资源类型查找

### Gateway

| 注解 | 值 | 说明 |
|------|---|------|
| `edgion.io/enable-http2` | `"true"\|"false"` | 启用 HTTP/2 |
| `edgion.io/backend-protocol` | `HTTP\|HTTPS\|H2\|H2C` | 后端协议 |
| `edgion.io/http-to-https-redirect` | `"true"\|"false"` | HTTP→HTTPS 重定向 |
| `edgion.io/https-redirect-port` | `port` | HTTPS 重定向端口 |
| `edgion.io/edgion-stream-plugins` | `ns/name` | StreamPlugins 绑定 |
| `edgion.io/metrics-test-key` | `string` | 测试用 Metrics 标签键 |
| `edgion.io/metrics-test-type` | `string` | 测试用 Metrics 标签值 |

### HTTPRoute / GRPCRoute

| 注解 | 值 | 说明 |
|------|---|------|
| `edgion.io/max-retries` | `u32` | 最大重试次数 |
| `edgion.io/hostname-resolution` | — | 系统管理（不要手动设置） |

### TCPRoute / TLSRoute

| 注解 | 值 | 说明 |
|------|---|------|
| `edgion.io/edgion-stream-plugins` | `ns/name` | StreamPlugins 绑定 |
| `edgion.io/proxy-protocol` | `"1"\|"2"` | Proxy Protocol 版本 |
| `edgion.io/upstream-tls` | `"true"\|"false"` | 上游 TLS |
| `edgion.io/max-connect-retries` | `u32` | 最大连接重试 |

### EdgionTls

| 注解 | 值 | 说明 |
|------|---|------|
| `edgion.io/expose-client-cert` | `"true"\|"false"` | 暴露客户端证书到请求头 |

### Service / EndpointSlice / Endpoints

| 注解 | 值 | 说明 |
|------|---|------|
| `edgion.io/health-check` | YAML 字符串 | 健康检查配置 |

## Options 键

### Gateway Listener `tls.options`

| 键 | 值 | 说明 |
|----|---|------|
| `edgion.io/cert-provider` | `"secret"\|"edgion-tls"` | 证书来源 |

### BackendTLSPolicy `spec.options`

| 键 | 值 | 说明 |
|----|---|------|
| `edgion.io/client-certificate-ref` | `ns/name` | 客户端证书 Secret |

## Labels

| Label | 值 | 说明 |
|-------|---|------|
| `edgion.io/leader` | pod name | Leader 选举标记 |
| `edgion.io/managed-by` | `"acme"` | ACME 管理标记 |
| `edgion.io/acme-resource` | resource name | ACME 资源关联 |

## 系统保留键

以下注解由系统自动管理，**不要手动设置**：

| 注解 | 说明 |
|------|------|
| `edgion.io/hostname-resolution` | Controller 注入的域名解析结果 |
| `edgion.io/sync-version` | 配置同步版本号 |

详细参考见 [references/](references/) 目录。
