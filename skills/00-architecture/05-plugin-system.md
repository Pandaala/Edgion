# 插件系统

> Edgion 的四阶段插件框架：RequestFilter → UpstreamResponseFilter → UpstreamResponseBodyFilter → UpstreamResponse。
> 插件在路由预解析时实例化（非请求时），通过 PluginRuntime 统一管理。

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

## Key Files

- `src/core/plugins/plugin_runtime/runtime.rs` — `PluginRuntime`
- `src/core/plugins/plugin_runtime/conditional_filter.rs` — condition wrapping
- `src/core/plugins/plugin_runtime/traits/` — all trait definitions
- `src/core/plugins/edgion_plugins/` — plugin implementations

## Related

- [Plugin Development Guide](../01-development/01-edgion-plugin-dev.md) — how to create new plugins
- [Stream Plugin Development](../01-development/02-stream-plugin-dev.md) — TCP-level stream plugins
- [Access Log / PluginLog](../02-observability/00-access-log.md) — plugin logging specification
