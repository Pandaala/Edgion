---
name: size-and-similarity-heuristics
description: 判定"按行数/表面相似度推 split / 抽公共"类 finding。需先验证三项前置条件，否则直接关闭。
---

# 行数 / 表面相似度驱动的重构建议 — 验收三件套

**审计日期**：2026-04-26
**关联 issue**：P2-11（已关闭，未修复）
**结论**：**仅"文件大 / 看起来类似"不构成 finding。提交此类建议前必须给出三项证据；否则直接关闭。**

## 三项必备证据

提"应拆 / 应抽公共"类 finding 时，**必须**在 finding 中明确：

1. **真实重复**：不是「都做某件事」，而是**行级近似**（同样的几十行代码块出现 ≥2 次）。「两个插件都写 429 响应」不是重复，「两个插件都用同样 30 行构造同样的 ResponseHeader」才是。
2. **真实耦合泄漏**：拆分目标的内部细节**已经被外部引用**（grep 出 ≥2 处跨模块直接 import 内部类型/常量），或类型层面已经分裂（如 enum variant 携带的字段彻底不同）。
3. **拆开后追踪成本下降**：读者按一条故事线（一次请求 / 一次 reconcile / 一次启动）跟踪时，**不需要在新拆出的多个文件之间反复跳转**。线性故事被横向切开是负价值。

三项缺一就是「按行数 / 看起来像」推动的形式重构，不接受。

## 已审计的具体点（本次扫描覆盖，未来不重提）

### (a) `src/types/resources/common/mod.rs`（124 行）"路由 vs LinkSys 混合"

- **前提错误**：`secret_object_ref` / `secret_slot` / `tls` 三子模块被 `gateway.rs`、`edgion_acme`、`EdgionTls`、`EdgionPlugins`、`link_sys` **全部资源类型共享**，并非 LinkSys 专用。
- **判定**：误报。124 行也远低于值得拆的阈值。

### (b) `src/types/mod.rs:53` `pub use self::resources::*` "新增资源自动成为对外 API"

- **前提不适用**：Edgion 是 binary 项目（Controller / Gateway / edgion-cli / Center），不发布 lib，没有"对外 API 稳定边界"概念。
- **判定**：不适用。binary 内部 glob re-export 是合理选择。

### (c) `edgion_acme.rs` parse "ResolveOutcome 全分支内联大 match"

- **已完成**：`outcome_to_resolved_refs_action(outcome, role, …)` 已存在（文件 ~322 行附近），`AcmeResolveCtx` / `AcmeResolveAction` enum 静态约束 Account vs Dns 语义。`parse()` 内只剩薄 dispatch + `update_condition`，正是当年 finding 想要的形态。
- **判定**：已修复，issue 漏关。

### (d) `rate_limit` vs `rate_limit_redis` "共同逻辑未抽取"

- **不通过验收**：两者 `add_headers` / `reject_request` 操作不同类型（`isize` vs `i64`、`Rate` 本地 CMS vs Redis Lua 脚本结果），可抽公共部分仅约 30 行 header 写入和 429 body 构建，且会把两个目前**完全独立**的插件强耦合。`get_header_names()` 的 helper 已经覆盖了真正可复用的部分。
- **判定**：插件独立性优先，不接受抽公共建议。

### (e) `src/core/gateway/runtime/store/config.rs`（622 行）"对照 `ConfigClient::required_cache_names` 改 `is_ready`"

- **引用不存在的 API**：`required_cache_names` 在仓库 grep 无结果；该文件也没有 `is_ready` 函数，单一职责为 `GatewayListenerConfig` 查询逻辑。
- **判定**：finding 前提错误，关闭。

### (f) `src/core/common/unified_auth/mod.rs`（808 行 = 559 非测试 + 249 测试）"应拆 state/middleware/oidc/local"

- **故事线完整**：types → state 构造 → middleware 主流程 → 两个 validator → helpers，线性可读，无重复无耦合泄漏。
- **拆开是负价值**：`try_oidc_validation` / `try_local_validation` 是 `AuthMiddlewareState` / `LocalAuthState` 的薄 adapter，外部不引用；拆出后读者理解一次请求要在 3 个文件间跳。
- **唯一勉强可做的小整理**：`UnifiedAuthState::from_configs`（~135 行启动期构造）可单独挪到 `state.rs`，理由是「启动期 vs 热路径」分离 —— 但收益微弱，不构成 P2/P3。
- **判定**：不接受拆分。项目内 559 行非测试代码不算大（参照 `edgion_acme.rs` 1491 行、`config.rs` 622 行）。

## 何时重新评估

- 上述任何文件出现**真实**的行级重复（≥2 处 30 行+ 近似块），重新评估 (d)。
- `unified_auth` 增加 ≥1 个新 provider（如 SAML），(f) 拆分价值显著上升。
- 新增可被 grep 验证的"对外 API 稳定承诺"机制，(b) 重新评估。

## 不接受的修复建议清单

- "文件 X 行太多，应拆分" —— 不附三项证据直接关闭。
- "两个模块看起来都做 Y，应抽公共" —— 不附 ≥2 处 30 行+ 行级近似证据直接关闭。
- "新增 X 自动成为对外 API" —— 不区分 binary / lib 项目直接关闭。
