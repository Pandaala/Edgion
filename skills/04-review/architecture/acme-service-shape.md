---
name: acme-service-shape
description: 判定 ACME `service.rs` "巨型阶段函数应拆 / 全局单例应改 DI" 类 finding。已审计为设计取舍，不接受重构类修复建议。
---

# ACME service.rs 形态 — 已审计取舍

**审计日期**：2026-04-26
**关联 issue**：P1-05（已关闭，仅修代码缺陷部分，不做架构重构）
**文件**：`src/core/controller/services/acme/service.rs`（~1158 行）

## 已修复（曾经的真实缺陷）

- 状态写回静默丢弃 → 现在 `if let Err(status_err) = update_acme_status(...)` 打 `tracing::warn!`（service.rs:395-409）
- DNS 凭据非 UTF-8 静默回退 base64 → 现在显式 `String::from_utf8(...).map_err(...)?` 返回 Err（service.rs:751）

新出现这两类（吞错 / 非 UTF-8 静默降级）才接受 finding。

## 不接受的修复建议（本次审计后）

### 1. "`execute_http01_flow` / `execute_dns01_flow` 90 行太长，应拆 phase 子函数"

- ACME 协议天然线性流程：account → order → challenge → finalize → certificate，每步依赖前一步结果。
- 拆出的子函数只被本流程各调用一次，参数列表长（client、acme、status、challenge state…），读者要在 5–6 个文件间反复跳。
- "线性故事被横向切开" 属负价值，命中 `architecture/size-and-similarity-heuristics.md` 验收三件套未过。
- **判定**：不接受拆分。

### 2. "`handle_resource_check`（~173 行）应拆"

- 大头是 `tokio::spawn` 内联块（成功/失败/重试调度三分支）。
- 抽成 `async fn run_acme_flow(...)` 是机械操作，行为不变，收益仅"少一层缩进"。
- 可做可不做，不构成 P1/P2 级 finding；提交此类建议需附"读者实际困惑"证据。

### 3. "`ACME_SERVICE: Mutex<Option<AcmeServiceHandle>>` 全局单例应换 `Arc<...>` 注入 HandlerContext"

- Controller 是 leader-elected 单活模型，多实例并发不在设计目标内。
- 唯一痛点是单测不便，但 ACME 流程瓶颈是 K8s API + 外部 ACME server，换实例化方式仍然测不了，只是把 mutex 搬家。
- 改造扩散到 HandlerContext + 所有 handler 签名，收益虚。
- **判定**：不接受 DI 重构。如果未来真要给 ACME 加单测或多租户拆分，再开新 issue 针对性做。

## 何时重新评估

- ACME service 真的要支持多实例（如多租户隔离）→ 重新评估 #3。
- 出现真实可单测的逻辑分层（如把 ACME state machine 抽成纯函数模块）→ 重新评估 #1#2 拆分价值。
- 出现新的吞错 / 静默降级（status 写回、外部 IO、密钥解析）→ 走"已修复"那一类正常 finding。
