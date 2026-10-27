# RequestBodyBuffer 插件

`RequestBodyBuffer` 在 `requestPlugins` 阶段显式捕获被代理的请求 body，并将其保存在请求级共享缓存中。之后的插件（例如位于 `upstreamResponseFilterPlugins` 的 DSL 条目）可在请求 body 已转发给上游之后读取这份缓存。

```yaml
spec:
  requestPlugins:
    - type: RequestBodyBuffer
      config: {}
      body:
        maxBodySize: 1MiB
  upstreamResponseFilterPlugins:
    - type: Dsl
      config:
        name: inspect-request-after-upstream
        source: |
          if req.body == "expected" {
            resp.set_header("X-Body-Matched", "true")
          }
        errorPolicy: deny
```

该插件只能放在 `requestPlugins`。它没有自身配置字段；条目级 `body` 块可选，用于增加条件和逻辑大小限制。捕获过程复用网关唯一的共享 `BodyStorage`，包括内存转磁盘和物理 body 上限机制。

`EdgionGatewayConfig.spec.requestBody` 控制是否允许捕获、默认内存溢写阈值和请求体物理上限；其中可选的最大值字段可覆盖对应默认值。禁用捕获时，该插件会以通用的 body 不可用响应固定 fail-close；无请求体的请求仍使用规范空 body。

晚阶段消费者绝不会新建 body 缓存。如果之前没有插件填充缓存——例如未配置
`RequestBodyBuffer`，或其 body 条件跳过了捕获——对于声明了 body 的请求，晚阶段
读取固定 fail-close：无论消费者的 `errorPolicy` 如何配置，都返回 HTTP 500 和
`Request body processing failed`。确实未声明 body 的请求仍会读取到规范空 body，
不需要缓存。

捕获失败发生在 VM 执行前的 body gate，并使用空响应体：请求体超过上限时返回
413，超时时返回 408，拒绝捕获或其他失败时返回 500。这与晚阶段 VM Body 操作无法
访问或处理已要求缓存时使用的固定通用 500 不同。

DSL 将缓存视为 opaque `req.body` 值。`byte_len()`、`contains()`、`matches()`、`sha256()` 和 `json_get()` 等方法无需完整复制成字符串即可检查请求体；只有 `read(offset, max_bytes)` 会为所请求的结果创建有界字符串。无论缓存位于内存还是已溢写到磁盘，所有方法的行为都相同。

需要完整请求 body 的 Wasm 模块通常应放在 `requestPlugins` 并导出 `proxy_on_request_body`；响应阶段的 Wasm 宿主阶段不会暴露这份请求 body 缓存。

请求体捕获及仍被引用的正文共享启动配置
[`gateway.request_body_budget`](../../../../ops-guide/configuration/gateway-config.md#进程级请求体存储预算)
的进程级额度。额度不足始终返回 HTTP 503，即使 DSL 或 Wasm 配置为忽略普通错误也不例外。
它与单请求超限的 413、存储 I/O 故障的 500 分别处理。
