# TraceContext 插件

> **🔌 Edgion 扩展**
>
> TraceContext 是 `EdgionPlugins` CRD 提供的 W3C Trace Context 传播插件，不属于标准 Gateway API。

## TraceContext 解决了什么问题？

当 OpenTelemetry 插装的服务网格位于网关之后时，每个后端服务都需要知道请求属于哪条分布式链路。W3C Trace Context 规范（[traceparent](https://www.w3.org/TR/trace-context/) 请求头）是跨服务边界关联 Span 的互操作标准。

TraceContext 插件让网关以**仅传播**的角色参与该标准：

- 网关读取并转发 `traceparent` / `tracestate` 请求头，保持入站上下文完整。
- 当入站请求不含有效上下文时，网关可以生成新的根 Span（全新 128 位 Trace ID）。
- 网关**不**向采集器导出 Span——它不是追踪后端。访问日志和响应头中的 `x-trace-id` 取自 `traceparent` 的 32 位十六进制 Trace ID。

该插件为**按需启用**：未在路由上挂载 `TraceContext` 插件时，网关直接透传请求头，不做任何解析。

## 快速开始

### 最简配置（透明透传 + 自动生成）

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: trace-context-default
  namespace: edgion-default
spec:
  requestPlugins:
    - type: TraceContext
      config:
        generateWhenMissing: true
        trustInbound: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: trace-context-route
  namespace: edgion-default
spec:
  parentRefs:
    - name: edgion-plugins-gateway
      namespace: edgion-test
  hostnames:
    - "trace-context.example.com"
  rules:
    - matches:
        - path: { type: PathPrefix, value: "/" }
      filters:
        - type: ExtensionRef
          extensionRef: { group: edgion.io, kind: EdgionPlugins, name: trace-context-default }
      backendRefs:
        - name: test-http
          namespace: edgion-test
          port: 30001
```

### 默认值说明

| 字段 | 默认值 | 含义 |
|------|--------|------|
| `generateWhenMissing` | `true` | 没有可信且有效的入站上下文时自动生成根 `traceparent` |
| `echoToClient` | `false` | 不将 `traceparent` 写入客户端响应 |
| `trustInbound` | `true` | 信任格式正确的入站 `traceparent`（透明透传） |
| `defaultSampled` | `false` | 生成根上下文时将 W3C sampled 建议位设为 `00` |

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `generateWhenMissing` | Boolean | `true` | 为 `true` 时，若不存在可信且有效的入站 `traceparent`（因为缺失、格式错误或被 `trustInbound: false` 丢弃），网关生成新的 128 位 Trace ID 并向上游注入合法的根 `traceparent`。为 `false` 时不生成根上下文；格式错误或不受信任的入站 Trace 请求头仍会被移除。 |
| `echoToClient` | Boolean | `false` | 为 `true` 时，将最终生效的 `traceparent`（透传或新生成的）写回发给客户端的 HTTP 响应。适用于需要将 UI 操作关联到后端链路的单页应用或移动端。 |
| `trustInbound` | Boolean | `true` | 为 `true` 时，格式正确的入站 `traceparent` 原样转发给上游（透明透传），网关将 `x-trace-id` 对齐到 W3C 32 位十六进制 Trace ID。为 `false` 时，网关移除入站 `traceparent` 和 `tracestate`，之后由 `generateWhenMissing` 决定是否生成新的根上下文。适合互联网边缘部署，防止客户端伪造 Trace 上下文。 |
| `defaultSampled` | Boolean | `false` | 控制网关生成的 `traceparent` 中 W3C sampled 建议位：`false` → trace-flags `00`；`true` → `01`。该位表达网关的采样建议，不会强制下游采样器接受。透传入站 `traceparent` 时此字段无效。 |

---

## 传播行为

### 情形 1 — 入站 traceparent 格式正确（`trustInbound: true`）

网关原样转发 `traceparent` 请求头给上游。`tracestate` 也同样转发，但若超过 512 字节则丢弃。网关访问日志中的 `x-trace-id` 以及可选的响应头对齐到 `traceparent` 中的 32 位十六进制 Trace ID。

### 情形 2 — 入站 traceparent 缺失或格式错误

若 `generateWhenMissing: true`（默认），网关生成新的根 `traceparent`（全新 128 位 Trace ID）并注入上游，该请求成为所有下游服务的链路根节点。客户端发来的残缺或垃圾 `traceparent` / `tracestate` 请求头在转发前被剥除。

若 `generateWhenMissing: false`，网关不生成根上下文。原本缺失的上下文仍保持缺失；格式错误的 `traceparent` 及其关联 `tracestate` 会在转发前被移除。

### 情形 3 — `trustInbound: false`（互联网边缘部署）

网关移除所有入站 `traceparent` / `tracestate`，无论其格式是否正确。若 `generateWhenMissing: true`，随后生成新的根 `traceparent`；若为 `false`，请求将保持没有 Trace 上下文。移除外部标识可防止客户端伪造 Trace 上下文，避免外部 ID 污染内部链路树。

---

## 安全说明——何时应设置 `trustInbound: false`

在面向互联网的边缘部署中，客户端可提交任意 `traceparent`，使内部链路看起来从外部 Span ID 派生。潜在风险包括：

- **日志污染**：外部 Trace ID 出现在访问日志和可观测性仪表盘中。
- **链路伪造**：伪造的 `traceparent` 可能使恶意请求看起来属于合法链路。

对公开路由或调用方不可信的场景，请设置 `trustInbound: false`。在内部服务网格路由（网关位于服务网格 Sidecar 之后）中，`trustInbound: true`（默认）是安全的，且可保留端到端链路关联。

---

## 插件链约束

### 每个路由最多一个 TraceContext

Controller 会拒绝在同一 `requestPlugins` 列表中声明多个 `TraceContext` 的 `EdgionPlugins` 资源。每个路由仅配置一个实例。

### 与 RequestMirror 的顺序

当同一插件链同时包含 `TraceContext` 和 [RequestMirror](./request-mirror.md) 时，必须将 `TraceContext` 声明在 `RequestMirror` **之前**。请求阶段插件按顺序执行后才会分发镜像请求；将 `TraceContext` 置于前面可确保镜像请求携带与主请求相同的对齐 `traceparent`。

```yaml
spec:
  requestPlugins:
    # 1. 先对齐或生成 Trace 上下文
    - type: TraceContext
      config:
        generateWhenMissing: true
    # 2. 镜像请求携带已对齐的 traceparent
    - type: RequestMirror
      config:
        mirrors:
          - backendRef:
              name: shadow-service
              port: 8080
```

---

## 范围与兼容性

本插件仅支持 W3C Trace Context 的 `traceparent` 请求头，**不**处理：

- Zipkin B3 请求头（`X-B3-TraceId`、`X-B3-SpanId`、`X-B3-Flags`、`b3`）
- Jaeger `uber-trace-id`
- AWS X-Ray `X-Amzn-Trace-Id`

但任何 OpenTelemetry 插装的下游服务——无论其后端导出目标是 Jaeger、Zipkin、Tempo、Datadog 等——均原生支持 W3C Trace Context，因此本插件可与所有兼容 OTLP 的可观测性栈完整互操作。

---

## 完整配置示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: trace-context-edge
  namespace: production
spec:
  requestPlugins:
    - type: TraceContext
      config:
        generateWhenMissing: true
        trustInbound: false   # 面向互联网：丢弃入站 Trace 上下文
        echoToClient: true    # 将 traceparent 写入响应，供客户端关联
        defaultSampled: false # 生成的根上下文携带 W3C sampled 位 00
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: trace-context-edge
      backendRefs:
        - name: api-service
          port: 8080
```

---

## 常见问题

### Q1：下游 Jaeger/Zipkin 界面未显示网关发起的 Span？

**A**：TraceContext 仅做传播，网关不导出 Span。它注入 `traceparent`，使下游服务记录的 Span 共享同一 Trace ID。请在下游服务的 OpenTelemetry SDK 中配置 Jaeger/Zipkin 导出器；网关的职责仅为启动和携带 Trace ID。

### Q2：设置了 `echoToClient: true`，但浏览器收不到该响应头？

**A**：请检查 CORS 策略（通过 [CORS 插件](./cors.md)）是否在 `Access-Control-Expose-Headers` 中暴露了 `traceparent`。浏览器会屏蔽不在暴露列表中的响应头。

### Q3：Controller 拒绝了我的 EdgionPlugins 资源？

**A**：最常见的原因是在同一 `requestPlugins` 列表中声明了多个 `TraceContext`。请删除重复项。

### Q4：是否应该使用 `defaultSampled: true`？

**A**：当网关生成的根上下文应携带 W3C sampled 建议位 `01` 时设为 `true`；默认的 `false` 写入 `00`。该字段只表达建议，不会强制下游采样器接受；下游行为仍由各自配置决定。

---

## 相关链接

- [Request Mirror 插件](./request-mirror.md)
- [CORS 插件](./cors.md)
- [过滤器目录](../README.md)
