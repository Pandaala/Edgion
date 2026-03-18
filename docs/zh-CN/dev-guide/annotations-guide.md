# Annotations 与 `edgion.io/*` 扩展键

本文档说明 Edgion 当前哪些 `edgion.io/*` 键属于真实的 `metadata.annotations`，哪些属于 `options` 或 `labels`，以及哪些是系统保留或测试专用键。

如果你是在改代码、排查问题或更新文档，建议先看 agent 侧入口：

- [05-annotations-reference.md](../../../skills/development/05-annotations-reference.md)

详细键表拆在这些 reference 里：

- [annotations-metadata.md](../../../skills/development/references/annotations-metadata.md)
- [annotations-options-and-labels.md](../../../skills/development/references/annotations-options-and-labels.md)
- [annotations-system-and-test-keys.md](../../../skills/development/references/annotations-system-and-test-keys.md)

## 先看“放在哪”，再看“叫什么”

同样是 `edgion.io/*` 前缀，配置位置并不一样：

| 位置 | 例子 | 说明 |
|------|------|------|
| `metadata.annotations` | `Gateway.metadata.annotations["edgion.io/enable-http2"]` | 最常见的扩展入口 |
| `listener.tls.options` | `listener.tls.options["edgion.io/cert-provider"]` | Listener TLS 扩展，不属于 annotations |
| `BackendTLSPolicy.spec.options` | `spec.options["edgion.io/client-certificate-ref"]` | Backend TLS 扩展，不属于 annotations |
| `metadata.labels` | `edgion.io/leader` | 调度/归属标签，不属于 annotations |

如果一开始就把位置看错，后面代码和文档基本都会跟着错。

## 当前高频 `metadata.annotations`

### Gateway

| Key | 作用 |
|-----|------|
| `edgion.io/enable-http2` | 控制 HTTP/2 支持 |
| `edgion.io/backend-protocol` | TLS listener 的后端协议扩展，当前常见值是 `"tcp"` |
| `edgion.io/http-to-https-redirect` | 非 TLS listener 上启用重定向 |
| `edgion.io/https-redirect-port` | 重定向目标端口 |
| `edgion.io/metrics-test-key` | 集成测试 metrics 关联键 |
| `edgion.io/metrics-test-type` | 集成测试 metrics 模式 |
| `edgion.io/edgion-stream-plugins` | Gateway 级连接过滤入口 |

### Route / TLS / Backend

| Key | 资源 | 作用 |
|-----|------|------|
| `edgion.io/max-retries` | `HTTPRoute` / `GRPCRoute` | 路由级重试覆盖，优先级高于全局配置，`0` 表示禁用 |
| `edgion.io/edgion-stream-plugins` | `TCPRoute` / `TLSRoute` | 解析 `EdgionStreamPlugins` 引用 |
| `edgion.io/proxy-protocol` | `TLSRoute` | 当前实现只识别 `"v2"` |
| `edgion.io/upstream-tls` | `TLSRoute` | 控制到上游是否走 TLS |
| `edgion.io/max-connect-retries` | `TLSRoute` | 上游连接重试次数 |
| `edgion.io/expose-client-cert` | `EdgionTls` | 把 mTLS 客户端证书信息暴露给插件层 |
| `edgion.io/health-check` | `Service` / `EndpointSlice` / `Endpoints` | 主动健康检查 YAML 配置 |

## 不是 annotations 的高频键

| Key | 实际位置 | 说明 |
|-----|----------|------|
| `edgion.io/cert-provider` | `Gateway.spec.listeners[*].tls.options` | Listener TLS 证书来源扩展 |
| `edgion.io/client-certificate-ref` | `BackendTLSPolicy.spec.options` | 上游 mTLS 客户端证书引用 |
| `edgion.io/leader` | `metadata.labels` | K8s HA 领导者标签 |
| `edgion.io/managed-by` | `metadata.labels` | ACME 等系统生成资源的归属标签 |
| `edgion.io/acme-resource` | `metadata.labels` | ACME 关联资源标签 |

## 系统保留与测试专用键

这些键可以在排障时读取，但通常不应该手工写进业务清单：

| Key | 类型 | 说明 |
|-----|------|------|
| `edgion.io/hostname-resolution` | 系统保留 annotation | Controller 在 `HTTPRoute` / `GRPCRoute` 上写入的诊断信息 |
| `edgion.io/sync-version` | 系统保留 annotation | 用于 control-plane 与 data-plane 关联 |
| `edgion.io/skip-load-validation` | 测试/工具 annotation | 供配置装载校验工具跳过特定 YAML |
| `edgion.io/force-sync` | 测试 annotation | 集成脚本用于强制触发 Secret 更新事件 |
| `edgion.io/trigger` | 运维 annotation | 手动触发 ACME 重新处理 |

## 容易踩坑的历史漂移

- 当前前缀是 `edgion.io`，不是 `edgion.com`。
- 当前 Gateway/TCPRoute/TLSRoute 使用的 stream-plugin 键是 `edgion.io/edgion-stream-plugins`。
- 仓库里旧文档和旧样例曾出现 `edgion.io/stream-plugins`，新改动不要继续沿用。
- `edgion.io/cert-provider` 和 `edgion.io/client-certificate-ref` 虽然长得像 annotation，但它们属于 `options` 字段。

## 相关文档

- [HTTP to HTTPS 重定向](../ops-guide/gateway/http-to-https-redirect.md)
- [后端主动健康检查](../user-guide/http-route/backends/health-check.md)
- [Stream Plugins 用户指南](../user-guide/tcp-route/stream-plugins.md)
- [knowledge-source-map.md](./knowledge-source-map.md)
