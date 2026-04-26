---
name: plugin-runtime-god-match
description: 判定 plugin_runtime.rs 中央 match 分发"god object/巨型文件"类 finding。已审计的设计取舍，不修。
---

# plugin_runtime.rs 中央 match 分发 — 已审计的设计取舍

**审计日期**：2026-04-26
**关联 issue**：P2-06（已关闭，未修复）
**结论**：**当前的中央 match 分发是刻意保留的，不接受"god object / 应改注册表 / 应用 enum_dispatch / 应分阶段拆文件"类修复建议。**

## 涉及代码

- `src/core/gateway/plugins/runtime/plugin_runtime.rs`（~987 行）
  - `create_request_filter_from_edgion`
  - `create_upstream_response_filter_from_edgion`
  - `create_upstream_response_body_filter_from_edgion`
  - `create_upstream_response_from_edgion`
  - `get_plugin_validation_error`
  - `get_plugin_name`

## 看上去像问题的点（都不修）

### 1. 一个文件 ~900 行，6 个并列的中央 match

`EdgionPlugin` 有 33 个 variant，每加一个 HTTP 插件需要在 `plugin_runtime.rs` 改 4 个位置（`use` + 构造 match + validation match + name match）。表面上是教科书式的 god object。

**为什么不改**：

- 这些 match 是**穷举式**的，少处理一个 variant Rust 编译器会在多处报错。**编译期穷举检查是核心安全保证**，禁止替换为运行时注册（inventory/linkme）。
- 中央 match 提供了三项实际价值：
  1. **插件全景一处可见**：打开一个文件即可看到「网关支持哪些 HTTP 插件、各属哪个 stage」。
  2. **stage 约束显式中央化**：`CreateResult::WrongStage` 显式表达了"哪个插件能进哪个 stage"的部署语义。trait 化（default impl 返回 WrongStage）会让这层约束**变得不可见**。
  3. **stage-specific 错误消息集中**：如 `"upstreamResponseFilterPlugins[{}] ({}): plugin is not valid for upstream response filter stage"`，集中维护比 33 个 trait impl 各写一遍清晰。

### 2. "应改 `enum_dispatch` + per-plugin trait 让新增插件零中央改动"

这条建议**性能上无收益**，DX 上也不一定划算：

- **不动热路径**：当前请求路径已经是 `Vec<Box<dyn RequestFilter>>` 上的 vtable 调用。`enum_dispatch` 重构只动**配置构建期的中央 match**（每次 reload 几十次，纳秒级），不影响请求路径。
- **要动热路径就是另一种重构**：把 `Vec<Box<dyn RequestFilter>>` 替换为 `Vec<RequestFilterPlugin>` 枚举。即使做了，vtable 节省 ~5-15ns/请求，相比 JWT 验签（~10-100µs）/ 正则匹配（~µs）/ Redis 限流（~ms）小 3-6 个数量级，**完全不是网关的瓶颈**。Pingora 自身大量使用 trait object 是同样的理由。
- **DX 净收益不明确**：33 个 variant 的机械迁移有不可见回归风险（trait impl 签名一致但内容写错时编译能过），且 trait 化后调试体验更分散（堆栈中是 `<XConfig as PluginConfigExt>::build_request` 而非中央函数）。

### 3. "应分阶段拆 4 个文件"

不解决核心痛点（新增插件依然需要改中央分发），只是把中央 match 拆到 4 个文件。属于**形式重构**，无实质改进。

## 何时重新评估

只有触发以下条件之一才考虑重写：

- 未来 3 个月新增 ≥ 5 个 HTTP 插件（重构成本可摊回）。
- 因其他原因（新 stage 模型、新 hook 点）正要重写 `plugin_runtime.rs`，可顺手做。
- 新人持续在 4 处中央修改上犯错。

否则，**新 finding 直接关闭**，引用本文。

## 不接受的修复建议清单

- "拆成插件注册表（inventory/linkme）"
- "用 `enum_dispatch` 把分发表分散到各插件文件"
- "按 stage 拆成 4 个文件"
- "提一个 `PluginConfigExt` trait 取代中央 match"
- "god object，应该重构"
- "新增插件需要改 4 处中央代码"（这是已知 trade-off，不是缺陷）

## 可接受的小幅改进（如果未来真要做）

- 用 `strum::IntoStaticStr` derive 替代 `get_plugin_name` 的手写 match（**减少 1 个**中央 match，保留其余结构）。
- 把 `get_plugin_validation_error` 收口到 trait `PluginConfig::validation_error`（**减少 1 个**中央 match，但每个 config 都得实现）。

这两个改动不打破中央化语义，单独评估。
