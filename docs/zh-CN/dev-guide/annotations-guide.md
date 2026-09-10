# Annotations 与 `edgion.io/*` 扩展键

本文档说明 Edgion 当前哪些 `edgion.io/*` 键属于真实的 `metadata.annotations`，哪些属于 `options` 或 `labels`，以及哪些是系统保留或测试专用键。

如果你是在改代码、排查问题或更新文档，建议先看 agent 侧入口：

- [05-annotations-reference.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/00-annotations-overview.md)

详细键表拆在这些 reference 里：

- [annotations-metadata.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-metadata.md)
- [annotations-options-and-labels.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-options-and-labels.md)
- [annotations-system-and-test-keys.md](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/05-annotations/references/annotations-system-and-test-keys.md)

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
| `edgion.io/http-to-https-redirect` | 非 TLS listener 上启用重定向 |
| `edgion.io/https-redirect-port` | 重定向目标端口 |
| `edgion.io/metrics-test-key` | 集成测试 metrics 关联键 |
| `edgion.io/metrics-test-type` | 集成测试 metrics 模式 |
| `edgion.io/edgion-stream-plugins` | Gateway 级连接过滤入口 |
| `edgion.io/inbound-proxy-protocol` | 在选定的 TCP 类 listener 上接收 PROXY protocol v1/v2；值为严格 JSON，见下文 |

#### 入站 PROXY protocol

内联信任列表：

```json
{"source":"inline","listeners":["https"],"config":{"mode":"trustedSources","trustedCidrs":["10.0.0.0/8","192.168.1.5"]}}
```

强制要求每条连接携带 header：

```json
{"source":"inline","listeners":["tcp-edge"],"config":{"mode":"required"}}
```

引用 `EdgionConfigData`：

```json
{"source":"resourceRef","listeners":["https"],"resourceRef":{"name":"edge-load-balancers","namespace":"networking"}}
```

`listeners` 可省略；省略时选择 Gateway 的全部 HTTP、HTTPS、TCP、TLS listener，并在混合
Gateway 中忽略 UDP。显式列表必须包含 1–64 个不重复且真实存在的 TCP 类 listener 名称；
显式选择 UDP 或在纯 UDP Gateway 上配置本注解均为非法。

`required` 会从任意 peer 解析 header，缺少或非法 header 时拒绝连接，仅适合网络边界已保证
所有连接均来自会生成 header 的负载均衡器的场景。`trustedSources` 仅在物理连接来源
`DirectPeerIp` 命中 `trustedCidrs` 时解析 header；未命中的连接保持普通直连语义，不能通过
伪造 header 改写 `RemoteIp`。裸 IP 会规范化为 `/32` 或 `/128`；空列表、非法 CIDR、
规范化后重复项以及 `/0` 会使整条策略无效，不会部分生效或退化为更宽的信任范围。

引用目标必须是已启用、没有 `spec.active` overlay、且 `data.type: ProxyProtocolTrust` 的
`EdgionConfigData`。关闭 ECD 引用校验时允许引用；开启时，同 namespace 始终允许，跨
namespace 的 `visibility: Cluster` 无需 grant，跨 namespace 的 `visibility: Namespace`
则需要从 `gateway.networking.k8s.io/Gateway` 到 `edgion.io/EdgionConfigData` 的精确
`ReferenceGrant`。

非法 JSON、listener 选择、策略、引用或共享物理 listener 冲突都会写入 Gateway conditions，
并在启动 Pingora 前 fail-close。启动后只要有效 socket 策略发生变化，readiness 会变为 503，
新 TCP 连接会被拒绝，Gateway 以非零状态快速退出，由 supervisor 使用最新有效快照重启。
`DirectPeerIp` 始终表示解析 header 前的物理 peer，永不被改写；可信 header 中的客户端地址
成为后续层使用的 `RemoteIp`。

### Route / TLS / Backend

| Key | 资源 | 作用 |
|-----|------|------|
| `edgion.io/edgion-stream-plugins` | `TCPRoute` / `TLSRoute` / `UDPRoute` | 解析 `EdgionStreamPlugins` 引用。UDPRoute 在接纳新会话时执行 `spec.plugins`，不使用 Gateway 监听器的 `ConnectionFilter` |
| `edgion.io/proxy-protocol` | `TLSRoute` | 当前实现只识别 `"v2"` |
| `edgion.io/max-connect-retries` | `TLSRoute` | 上游连接尝试次数，默认 2 次，最高 5 次 |
| `edgion.io/tcp-keepalive-time` | `TCPRoute` / `TLSRoute` | 上游 L4 连接在发送首个 keepalive 探针前的空闲秒数（正整数，1–7200）。设置该值即启用 keepalive；不设置则禁用（默认行为） |
| `edgion.io/tcp-keepalive-interval` | `TCPRoute` / `TLSRoute` | 相邻 keepalive 探针之间的间隔秒数（1–75）。仅在设置了 `tcp-keepalive-time` 时生效；缺失或非法值 = 75（操作系统默认值） |
| `edgion.io/tcp-keepalive-probes` | `TCPRoute` / `TLSRoute` | 连续探测失败多少次后判定连接已断开（1–20）。仅在设置了 `tcp-keepalive-time` 时生效；缺失或非法值 = 9（操作系统默认值） |
| `edgion.io/expose-client-cert` | `EdgionTls` | 把 mTLS 客户端证书信息暴露给插件层 |

Service 级别的后端行为（主动健康检查、异常点检测/熔断、panic 阈值、负载均衡算法）**不再是** annotation，而是通过 `EdgionBackendTrafficPolicy` CRD 配置，经由 `spec.targetRefs` 关联到 Service。详见
[`skills/02-features/03-resources/20-edgion-backend-traffic-policy.md`](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md)（英文，面向 AI Agent/开发者的完整 schema 参考），或用户指南：
[后端主动健康检查](../user-guide/http-route/backends/health-check.md)、
[熔断器](../user-guide/http-route/backends/circuit-breaker.md)、
[负载均衡算法](../user-guide/http-route/lb-algorithms.md)。

## 不是 annotations 的高频键

| Key | 实际位置 | 说明 |
|-----|----------|------|
| `edgion.io/cert-provider` | `Gateway.spec.listeners[*].tls.options` | Listener TLS 证书来源扩展 |
| `edgion.io/client-certificate-ref` | `BackendTLSPolicy.spec.options` | 上游 mTLS 客户端证书引用 |
| `edgion.io/leader` | `metadata.labels` | K8s HA 领导者标签 |
| `edgion.io/managed-by` | `metadata.labels` | ACME 等系统生成资源的来源标记；系统不读取该标签 |
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
- 当前 Gateway/TCPRoute/TLSRoute/UDPRoute 使用的 stream-plugin 键是 `edgion.io/edgion-stream-plugins`。
- 仓库里旧文档和旧样例曾出现 `edgion.io/stream-plugins`，新改动不要继续沿用。
- `edgion.io/cert-provider` 和 `edgion.io/client-certificate-ref` 虽然长得像 annotation，但它们属于 `options` 字段。

## 相关文档

- [HTTP to HTTPS 重定向](../ops-guide/gateway/http-to-https-redirect.md)
- [后端主动健康检查](../user-guide/http-route/backends/health-check.md)
- [Stream Plugins 用户指南](../user-guide/tcp-route/stream-plugins.md)
- [knowledge-source-map.md](./knowledge-source-map.md)
