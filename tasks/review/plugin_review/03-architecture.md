# Architecture Issues

## ARCH-01 [Important] `_ => None` 通配符屏蔽编译时 exhaustive match 检查

**模块**: `src/core/gateway/plugins/runtime/plugin_runtime.rs:392, 413, 477`

**问题**: `create_request_filter_from_edgion`、`create_upstream_response_filter_from_edgion`、`get_plugin_validation_error` 三个工厂/匹配函数均使用 `_ => None` 作为 catch-all。当新增 `EdgionPlugin` variant 时，编译器不会报 exhaustive match 错误，新 variant 静默落入 `None`。

这是 COR-04 (UrlRewrite 被丢弃) 的根本原因。

**影响**: Rust enum 的核心安全机制被绕过。新插件上线容易遗漏，且不会有编译期提示。

**修复建议**: 将 `_ => None` 替换为显式列举所有不适用的 variant：

```rust
// create_request_filter_from_edgion 中:
// 响应阶段专属插件 — 不适用于请求阶段
EdgionPlugin::ResponseHeaderModifier(_)
| EdgionPlugin::DebugAccessLogToHeader(_)
| EdgionPlugin::ResponseRewrite(_)
| EdgionPlugin::BandwidthLimit(_) => None,
```

添加新 variant 后编译器会强制要求处理每个 match arm。

---

## ARCH-02 [Observation] PluginRuntime::clone() 返回空实例

**模块**: `src/core/gateway/plugins/runtime/plugin_runtime.rs:71-76`

**问题**: `Clone` trait 实现返回 `Self::new()` 而非真正克隆。这违反了 `Clone` 的语义契约（clone 应等价于 original）。

**影响**: 依赖 `clone()` 获取运行时副本的代码会得到空实例。当前设计依赖 `preparse()` 重建，但这是一个 footgun。

**修复建议**: 考虑使用 newtype wrapper 或 `PluginRuntimeBuilder` 模式，而非实现带非标准语义的 `Clone`。如果保持现状，添加醒目文档。

---

## ARCH-03 [Observation] EdgionPlugin enum 无阶段约束

**模块**: `src/types/resources/edgion_plugins/edgion_plugin.rs:25-101`

**问题**: 所有 entry 类型 (`RequestFilterEntry`, `UpstreamResponseFilterEntry` 等) 使用同一个 `EdgionPlugin` enum 作为 `plugin` 字段。无类型级别约束阻止将请求阶段插件（如 `BasicAuth`）放入 `upstream_response_filter_plugins`。

**影响**: 错误放置的插件在运行时被静默忽略（factory 返回 `None`），用户无任何错误提示。

**修复建议**:
- 短期：在 `preparse()` 中添加阶段验证，当插件放在不支持的阶段时输出 warning
- 长期：按阶段使用独立 enum（如 `RequestPlugin`, `ResponseFilterPlugin`）

---

## ARCH-04 [Observation] Entry 类型代码重复

**模块**: `src/types/resources/edgion_plugins/entry.rs:48-393`

**问题**: `PluginEntry`, `RequestFilterEntry`, `UpstreamResponseFilterEntry`, `UpstreamResponseBodyFilterEntry`, `UpstreamResponseEntry` 五个结构体结构完全相同（相同字段、相同方法），只有名字不同。

**影响**: 维护负担——修改 entry 结构需要在五处同步修改。

**修复建议**: 使用泛型 `StageEntry<Stage>` + phantom type 或宏生成。

---

## ARCH-05 [Observation] Stream plugin 双重 ConfHandler 实现

**模块**: `src/core/gateway/plugins/stream/stream_plugin_store.rs:104-194`

**问题**: `ConfHandler<EdgionStreamPlugins>` 对 `Arc<StreamPluginStore>` 和 `&'static StreamPluginStore` 有两个几乎相同的实现。

**影响**: 逻辑变更需要同步更新两处。

**修复建议**: 在具体类型 `StreamPluginStore` 上实现，让两个包装类型委托调用。

---

## ARCH-06 [Minor] TCPRouteRule 中存在 dead field

**模块**: `src/types/resources/tcp_route.rs:61-65`

**问题**: `TCPRouteRule` 有 `stream_plugin_runtime: Arc<StreamPluginRuntime>` 字段，从不被赋值（始终 `Default::default()`）也从不被读取。实际插件执行通过 `stream_plugin_store_key` 做动态查找。

**影响**: 增加认知负担，误导开发者以为使用了嵌入式 runtime。

**修复建议**: 删除此 dead field。
