# RequestId 插件

> **Edgion 扩展**：通过 `EdgionPlugins` 配置 RequestId。

网关默认处理 W3C `traceparent` 和 `tracestate`，不需要挂载插件。合法入站上下文保持不变；缺失或无效时创建新上下文，生成时 sampled 位为 false。上下文传播不会自动开启 span 记录或 OTLP 导出。

默认保留合法的入站 `x-trace-id`，缺失时使用 W3C trace ID（32 位小写十六进制），向上游转发并回传响应。已有自定义 ID 可以与 W3C ID 不同。这些 ID 都不能用于身份认证。

只有一个 **RequestId 插件**，用于调整默认 ID 或增加业务关联 ID。`RequestTraceId` 是内部请求数据结构，不是插件或资源。旧 TraceContext 插件已移除。

## 快速开始

以下配置保留默认 `x-trace-id`，并增加 `x-order-id`。示例假设 `production` 中已存在引用的 Gateway 和 Service。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: request-ids
  namespace: production
spec:
  requestPlugins:
    - type: RequestId
      config:
        ids:
          - header: x-order-id
            source:
              type: header
              name: x-business-id
            generateWhenMissing: true
            generator:
              type: uuidV4
              prefix: order-
            forwardUpstream: true
            echoResponse: true
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - api.example.com
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: request-ids
      backendRefs:
        - name: api-service
          port: 8080
```

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|---|---|---|---|
| `defaultId.enabled` | Boolean | 保持前层设置；初始 true | 开启或关闭隐式 `x-trace-id` |
| `defaultId.forwardUpstream` | Boolean | 保持前层设置；初始 true | 向上游转发隐式默认 ID |
| `defaultId.echoResponse` | Boolean | 保持前层设置；初始 true | 回传隐式默认 ID |
| `ids` | Array | 空 | 按顺序执行的自定义规则 |
| `ids[].header` | String | 必填 | 目标头名称，不区分大小写 |
| `ids[].source` | KeyGet | 无 | `traceId`、带 `name` 的 `header`，或仅路由层可用的带 `name` 的 `ctx` |
| `ids[].generateWhenMissing` | Boolean | true | 目标和来源都没有合法值时是否生成 |
| `ids[].generator.type` | Enum | `uuidV4` | `uuidV4`、`uuidV7`、`randomHex`、`nanoId`、`md5` |
| `ids[].generator.prefix` | String | 空 | 只作用于新生成的值 |
| `ids[].generator.bytes` | Integer | 16 | 仅用于 `randomHex`；8–64 个随机字节 |
| `ids[].generator.length` | Integer | 21 | 仅用于 `nanoId`；输出长度 8–128 |
| `ids[].forwardUpstream` | Boolean | true | 向上游转发该 ID |
| `ids[].echoResponse` | Boolean | true | 响应回传该 ID |

`md5` 对新生成的随机字节取摘要，用于传统关联格式，不是认证机制。前缀最多 64 字节；ID 值须为 1–256 字节的可打印 ASCII，不能包含空白或逗号。重复的目标头或非法值不会被视为合法入站 ID。

目标名称最多 128 字节。认证凭据、路由、消息分帧、协议和 W3C 头不可作为目标，敏感 header/ctx 来源也被禁止。单份配置最多 16 条目标名称不同的规则；整个请求最多管理 16 个名称，包含已抑制的名称，活动值合计最多 4096 字节。保留默认 ID 时，应给它留出一个名额。

## 执行顺序与多网关

这里的生命周期适用于主 HTTP 代理，包括 gRPC 和 WebSocket 握手。独立的 HTTP 到 HTTPS 重定向服务不执行 RequestId 插件链；HTTP 头解析完成前或请求上下文建立前的失败，也无法保证回传 ID。

每条规则先保留目标头的第一个合法值；缺失时读取 `source`；仍缺失或无效时，由 `generateWhenMissing` 决定是否生成。前缀不会修改继承或复制的值。向上游转发所选值后，后续网关可以继续保留它。

核心默认值和全局规则在普通请求校验前执行。TLS 元数据或唯一监听器归属能确定 Gateway 时，Gateway 规则也在早期执行。共享监听器归属不明确时，先使用核心和全局 ID，路由确定归属后再执行 Gateway 规则，早于预检和后续插件。不会为格式错误或未匹配请求随意选择 Gateway。

路由规则在普通请求阶段按插件顺序执行，与已有 ID 增量合并。前面的插件提前拒绝后，后面的路由规则不会执行。不支持 BackendRef 挂载。早期来源仅支持 header/traceId，条件仅支持基于 header/method/traceId 的键条件；body、dye、路由上下文等不可用条件会被拒绝。

设置 `defaultId.enabled: false` 可去掉隐式默认值，再用 `ids` 添加自己的 ID，也允许显式配置 `x-trace-id`。后续关闭隐式默认值，不会删除已显式配置的自定义规则。关闭后的默认 ID 不再转发、回传或记录到关联 ID 日志，上游回显也不能恢复它。删除只作用于当前网关：下一跳看到头缺失时仍可能生成自己的默认 ID。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: custom-trace-id
  namespace: production
spec:
  requestPlugins:
    - type: RequestId
      config:
        defaultId:
          enabled: false
        ids:
          - header: x-correlation-id
            source:
              type: traceId
            echoResponse: true
```

## 响应与日志

网关不自动回传 `traceparent` 或 `tracestate`。通过默认 `x-trace-id`，或 `source.type: traceId` 的自定义规则回传 trace ID。这是关联响应头，不是 W3C 响应传播协议。跨域浏览器读取时，需在 [CORS 策略](./cors.md) 中暴露对应响应头。

`echoResponse: false` 会移除该受管理的响应头，包括上游回显；`forwardUpstream: false` 会从上游请求中移除它。两者都不会关闭 W3C 传播。

访问日志包含 `request_info.trace_id.w3c.trace_id` 和按头名称组织的 JSON 对象 `request_info.trace_id.correlation_ids`。复制的客户端/header/ctx 值在日志中脱敏，转发和响应值不变。生成值与标准 W3C ID 可用于关联；自定义值脱敏时，可按 W3C 字段查找。OTLP 日志导出仍需单独配置。

## 相关链接

- [访问日志](../../../../ops-guide/observability/access-log.md)
- [请求镜像](./request-mirror.md)
- [过滤器概览](../README.md)
