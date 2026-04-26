# ConfCenter etcd 抽象边界 — 已审计的设计取舍

**审计日期**：2026-04-26
**关联 issue**：P1-09 (已关闭，未修复)
**结论**：**已知设计取舍，不修复**。下一次 code review 看到这些"问题"时，先读这份文档再决定是否新开 issue。

## 涉及代码

- `src/core/controller/conf_mgr/conf_center/etcd/config.rs:140-211`
  （`EtcdConfig::to_client_config`）
- `src/core/controller/conf_mgr/conf_center/etcd/center.rs:47-48`
  （`EtcdCenter::new` 直接 new `EtcdLinkClient`）
- `src/core/gateway/link_sys/providers/etcd/config_mapping.rs:38`
  （`build_connect_options` 签名同时服务 CRD 流和 TOML 流）

## 看上去像问题的三个点（都不修）

### 1. ConfCenter 合成假 `Secret` / `SecretObjectReference`（"抽象泄漏"）

`config.rs:140-211` 直接 `use k8s_openapi::Secret` 和 `link_sys::TlsConfig`，并构造占位 `secret_ref`（`name: "conf-center-toml"`）+ 内嵌合成 `Secret`。

**为什么这样写**：复用 Gateway 端 `build_connect_options` 处理 TLS/auth 的代码路径，避免写第二份处理逻辑。代码注释已经明确标注 *"synthetic placeholder — secret_ref not used by the provider"*。

**为什么不修**：
- 只有 2 个调用方（CRD 流 + TOML 流），没有第三种连接来源出现
- 修复要重写 `build_connect_options` 签名 + 设计 `EtcdLinkClient` 重连语义（重连路径 `client.rs:126, 205` 复用了存下来的 `EtcdClientConfig`，改 DTO 后要想清楚 Secret 更新如何注入）
- 这恰是刚修过的 I4 静默回归（参考 `to_client_config` 注释里 "regression of the fix-change effort"）— 为"优雅"碰一个刚修好的关键路径不划算
- 没有运行时风险（不是内存/数据泄漏，纯设计气味）

**何时再考虑修**（触发条件，任一满足即重启此讨论）：
- 出现第 3 种连接来源（Vault / Consul / 其他 KV）
- 有人要修改 `Secret` 或 `link_sys::TlsConfig` 字段结构
- 因抽象泄漏出现了真实 bug

### 2. 构造用 `anyhow::Result` vs writer 用 `ConfWriterError`（"错误类型不一致"）

**不是不一致，是分层**：
- 构造期错误（`EtcdCenter::new` / `to_client_config`）：操作员看上下文/堆栈即可，`anyhow` 合适
- 运行期 writer 操作（`CenterApi`）：调用方需要区分 NotFound / Conflict 等，typed error 合适

Rust 生态常见模式。**不要再把这个标为问题**。

### 3. `EtcdLinkClient` 未 trait 化（"难以 mock"）

**YAGNI**：
- `EtcdLinkClient` 全仓只有一个实现
- writer 层已由 `EtcdStorage` + `CenterApi` trait 提供抽象（这才是 ConfCenter 的真实抽象边界）
- 没有"替换后端"的现实需求；提前抽 trait 只增加维护成本

**不要再把这个标为问题**，除非要真正 mock 它（例如新增 fault injection 测试）。

## 给未来 reviewer 的提示

如果你正在审计 `conf_center/etcd/` 并打算开"抽象泄漏 / 错误类型不一致 / 接口未抽象"类 issue —— 先确认你的论据是否超出本文档已覆盖的 3 个点。如果只是同样的论据，直接关闭你的 issue 草稿。
