---
name: style-consolidation-thresholds
description: 判定"将分散的小辅助 / 不同风格收拢到一处"类 finding。三行平凡函数复制不是问题；语义不同的 validate_* 风格是有意区分。
---

# 平凡 helper 抽取 / 风格统一 — 验收门槛

**审计日期**：2026-04-26
**关联 issue**：P3-04（已关闭，仅做了死链注释清理 + fed_sync 模块注释，其余项不接受）
**结论**：复制本身不是问题。"看起来重复"或"风格不一致"必须给出**真实维护成本**或**阅读成本**证据，否则关闭。

## 不接受的 finding

### (a) "三行平凡 helper 在多文件复制，应抽公共"

典型例子：`fn default_true() -> bool { true }`、`fn is_true(v: &bool) -> bool { *v }` 散落在 `src/types/resources/**` 13+ 处。

**为什么不接受**：

1. 函数本身 0 维护成本（永远不会改）。
2. 抽到 `resources::common::serde_defaults` 后每个调用站点反而要 `use ...::default_true;` —— 总行数不减反增。
3. 系统提示明确："Three similar lines is better than a premature abstraction"。

**例外**：helper 出现行为差异（一处 `true` 一处 `false`，本应同义却语义漂移），或 helper 本身复杂（≥10 行 + 可能演化）—— 此时抽取是为了**防止漂移**，不是消重复。

### (b) "`Option<&str>` 单错误 vs `Vec<String>` 聚合错误两种 validate 风格应统一"

`WebhookServiceConfig::get_validation_error -> Option<&str>` 与 `LinkSys::validate_config -> Vec<String>`。

**为什么不接受**：

1. 语义本来就不同：内层结构（webhook config）只关心"我自己合法吗" → first-error fail-fast；外层资源（LinkSys）需要把所有 variant 的错误聚合给 condition message → `Vec<String>`。
2. 当前设计正是把 `get_validation_error()` 的结果 `push` 进 `Vec` —— 两个角色清晰分工。
3. 强行统一只能向 `Vec<String>` 靠拢，但内层永远只产出 0 或 1 个元素，等于多写一行 `vec![]` / `vec![s]`，零信息增量。

**例外**：内层 validate 真的能产出多个独立错误（每个字段一条）时，改返 `Vec<String>`；目前 webhook 只做 6 处 if-return-Some，自然 first-error。

### (c) "中英文注释混用应统一"

项目惯例（来自实际阅读 `secret_ref.rs` / `link_sys/common.rs` 等多处）：**对外 API doc 倾向英文，模块/内部说明可中文**。审计模板里的"统一为某一种"建议忽略了这一惯例 —— 内部说明翻成英文反而损失精度。

**例外**：同一段注释中英夹杂（同句子里切语言），算病；不同段落不同语言，是惯例。

## 已做的清理（避免重提）

- 注释中散落的 `tasks/fix-change/I*/M*/R*/C1/D2/S3` 等外部任务编号（链接已失效）已在以下文件移除，保留实质语义：
  - `src/core/gateway/link_sys/credential.rs`
  - `src/types/resources/common/tls.rs`
  - `src/types/credentials/materials.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs`
  - `src/core/controller/conf_mgr/conf_center/etcd/config.rs`
- `src/core/common/fed_sync/mod.rs` 添加了模块级注释，说明仅 proto 类型，运行时逻辑分别在 `controller::fed_sync` / `center::fed_sync`。

未来若再扫到 `fix-change` 字符串或孤立的 `I4/M6/C1` 等代号 —— 那是新引入的，按死链处理；不必把已清理过的 6 个文件再扫一遍。
