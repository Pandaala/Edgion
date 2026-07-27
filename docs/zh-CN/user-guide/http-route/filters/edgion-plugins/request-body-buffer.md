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

晚阶段消费者绝不会新建 body 缓存。如果之前没有插件填充缓存——例如未配置 `RequestBodyBuffer`，或其 body 条件跳过了捕获——对于声明了 body 的请求，晚阶段读取会按照消费者自身的错误策略失败。确实未声明 body 的请求仍会读取到规范的空 body，不需要缓存。捕获拒绝和物理上限失败仍固定为 fail-close。

需要完整请求 body 的 Wasm 模块通常应放在 `requestPlugins` 并导出 `proxy_on_request_body`；响应阶段的 Wasm 宿主阶段不会暴露这份请求 body 缓存。
