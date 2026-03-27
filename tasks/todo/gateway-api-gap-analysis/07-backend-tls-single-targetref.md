# BackendTLSPolicy 强制单个 targetRef

## 优先级: P2
## 工作量: 小（预计 0.5 天）
## 类型: 规范对齐

---

## 问题描述

Gateway API v1.5 要求 BackendTLSPolicy 的 `targetRefs` 只能包含单个条目。
Edgion 当前只是 warn 日志但允许多个 targetRef，未严格执行 v1.5 要求。

## 当前状态

- `targetRefs` 字段类型为 `Vec<LocalPolicyTargetReferenceWithSectionName>`
- Controller 在 `backend_tls_policy.rs:45-53` 检测到多 targetRef 时仅输出警告
- 实际处理逻辑会处理所有 targetRef（不拒绝）

## 涉及文件

1. `src/types/resources/backend_tls_policy.rs` — 类型定义
2. `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/backend_tls_policy.rs:45-53` — 校验逻辑

## 实现建议

1. 当 `targetRefs.len() > 1` 时，设置 `Accepted: False`，reason: `Invalid`，message: `only one targetRef is allowed per BackendTLSPolicy`
2. 只处理第一个 targetRef，忽略其余
3. 或者：保持当前宽松行为但在状态中报告 warning condition（向前兼容）

### 推荐方案
考虑到向后兼容，建议采用"处理第一个，状态中报告警告"的方式：
- 处理 `targetRefs[0]`
- 状态条件中设置 `Accepted: True` 但 message 中注明 `only first targetRef is applied, additional targetRefs are ignored per Gateway API v1.5`

## 参考 Skills

- `skills/01-architecture/05-resources/SKILL.md` — 资源系统
- `skills/01-architecture/01-controller/SKILL.md` — Controller 架构

## 验证标准

- 单个 targetRef：行为不变
- 多个 targetRef：仅第一个生效，状态中有明确提示
- `cargo check --all-targets` 通过
