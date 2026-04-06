# Step 03: 冲突检测逻辑 ✅

> **Gap 3（致命）**：缺少冲突检测，`BackendTLSPolicyConflictResolution` 测试完全无法通过。

## Conformance 要求

`BackendTLSPolicyConflictResolution` 测试要求：

1. 多个 policy 指向同一 Service（无 sectionName）时，只有第一个被 Accepted
2. 后续 policy 的 status 为 `Accepted=False, Reason=Conflicted`
3. 同一 Service 的不同 sectionName 不冲突

## 实现方案

### 核心数据结构

新增 `backend_tls_policy_ref.rs`，复用 Controller 侧已有的 `BidirectionalRefManager<ResourceRef>` 关联表模式（与 `ServiceRefManager`、`SecretRefManager` 同一 pattern）：

- **正向索引**: `target_conflict_key → Set<PolicyRef>` — 某个 target 有哪些 policy
- **反向索引**: `policy_key → Set<target_conflict_key>` — 某个 policy 的 targets
- **时间戳存储**: `HashMap<policy_key, timestamp_string>` — 用于优先级排序

**Conflict key 格式**: `"{ns}/{svc_name}"` 或 `"{ns}/{svc_name}::{sectionName}"`
- 不同 sectionName = 不同 key = 不冲突（满足 conformance 要求 #3）

### 优先级规则

Gateway API 标准优先级（`is_policy_winner()` 实现）：
- 先按 `.metadata.creationTimestamp` 升序（更早创建的优先）
- 相同时间按 `{namespace}/{name}` 字典序（字母靠前的优先）
- 缺失 timestamp 的 policy 排最后

### Handler 生命周期集成

| 阶段 | 行为 |
|------|------|
| **parse** | 计算 old siblings → 清除旧 target 注册 → 注册新 conflict key → 存储 creationTimestamp → requeue old siblings |
| **update_status** | 遍历 target refs，调用 `is_policy_winner()`；非 winner → `Accepted=False, Reason=Conflicted`；validation error 优先于 conflict |
| **on_change** (新增) | requeue 当前 siblings，确保它们的冲突状态被重新评估 |
| **on_delete** (扩展) | 找到 siblings → 清除 target refs + timestamp → 清除 secret refs → requeue siblings |

### Sibling requeue 覆盖分析

| 场景 | old siblings | new siblings | 覆盖 |
|------|-------------|-------------|------|
| 创建 policy | 无 | on_change requeue | ✅ |
| 更新不改 target | parse requeue = on_change requeue | workqueue 合并 | ✅ |
| 更新改 target | parse requeue old | on_change requeue new | ✅ |
| 删除 policy | on_delete requeue | 无 | ✅ |

### Alpha3 兼容

`BackendTlsPolicyAlpha3Handler` 内部委托 v1 handler（`into()` 转换），所有冲突逻辑自动继承。

## 变更文件

| 文件 | 变更 | 说明 |
|------|------|------|
| `resource_processor/backend_tls_policy_ref.rs` | **新建** | target ref manager + timestamp store + 冲突判定辅助函数 + 8 个单元测试 |
| `resource_processor/mod.rs` | 修改 | 模块声明 + re-export |
| `resource_processor/status_utils.rs` | 修改 | 添加 `POLICY_CONFLICTED = "Conflicted"` reason 常量 |
| `resource_processor/handlers/backend_tls_policy.rs` | 修改 | parse/update_status/on_change/on_delete 全部集成冲突逻辑 |

## 验证

- `cargo check --all-targets` — 通过
- `cargo clippy --all-targets` — 无新 warning
- `cargo test --all` — 1644 passed, 0 failed
- 8 个新增冲突检测专项单元测试全部通过
- `BackendTLSPolicyConflictResolution` conformance 测试待集成测试环境验证
