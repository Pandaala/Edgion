# 插件系统

> Edgion 的插件目录现在分成三层：`plugins/http` 放 HTTP 插件实现，`plugins/stream` 放 TCP/UDP 侧 stream 插件，`plugins/runtime` 放执行框架、条件系统和 Gateway API filter adapter。
> 核心执行阶段仍然是四阶段插件框架：RequestFilter → UpstreamResponseFilter → UpstreamResponseBodyFilter → UpstreamResponse。

## Plugin Stages

Four plugin stages, each with its own trait:

| Trait | Timing | Async | Signature |
|-------|--------|-------|-----------|
| `RequestFilter` | Before upstream | Yes | `run_request(&self, session, log) → PluginRunningResult` |
| `UpstreamResponseFilter` | After upstream headers | No | `run_upstream_response_filter(&self, session, log) → PluginRunningResult` |
| `UpstreamResponseBodyFilter` | Per body chunk | No | `run_upstream_response_body_filter(&self, body, eos, session, log) → Option<Duration>` |
| `UpstreamResponse` | After upstream (full) | Yes | `run_upstream_response(&self, session, log) → PluginRunningResult` |

## Plugin Chain Execution (PluginRuntime)

```rust
// run_request_plugins: runs all RequestFilter plugins in order
for plugin in &self.request_filters {
    let result = plugin.run_request(session, log).await;
    match result {
        GoodNext | Nothing => continue,
        ErrTerminateRequest => { ctx.plugin_running_result = ErrTerminateRequest; break; }
        ErrResponse { .. } => { ctx.plugin_running_result = result; break; }
    }
}
```

## Conditional Wrapping

All plugins are automatically wrapped in `ConditionalRequestFilter` / `ConditionalUpstreamResponseFilter` which evaluates skip/run conditions before executing the plugin.

## Plugin Preparse

`PluginRuntime` is built during HTTPRoute/GRPCRoute preparse (not at request time), stored on the route rule. Plugin instantiation happens once per config change, not per request.

## Directory Layout

- `src/core/gateway/plugins/http/` — EdgionPlugins HTTP plugin implementations
- `src/core/gateway/plugins/stream/` — EdgionStreamPlugins runtime and connection filter bridge
- `src/core/gateway/plugins/runtime/runtime.rs` — `PluginRuntime` core
- `src/core/gateway/plugins/runtime/conditions/` — conditional execution (`PluginConditions`, evaluator)
- `src/core/gateway/plugins/runtime/gateway_api_filters/` — Gateway API filter adapters like `ExtensionRef`, `RequestHeaderModifier`
- `src/core/gateway/plugins/runtime/traits/` — all plugin trait definitions

## Key Files

- `src/core/gateway/plugins/runtime/runtime.rs` — `PluginRuntime`
- `src/core/gateway/plugins/runtime/conditional_filter.rs` — condition wrapping
- `src/core/gateway/plugins/runtime/conditions/` — condition model and evaluator
- `src/core/gateway/plugins/runtime/gateway_api_filters/` — Gateway API filter adapters
- `src/core/gateway/plugins/runtime/traits/` — all trait definitions
- `src/core/gateway/plugins/http/` — plugin implementations
- `src/core/gateway/plugins/stream/` — stream plugin implementations and bridge

## Related

- [Plugin Development Guide](../02-development/01-edgion-plugin-dev.md) — how to create new plugins
- [Stream Plugin Development](../02-development/02-stream-plugin-dev.md) — TCP-level stream plugins
- [Access Log / PluginLog](../03-coding-standards/observability/00-access-log.md) — plugin logging specification
