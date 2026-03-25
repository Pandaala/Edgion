# 状态条件与上报完善

优先级: P0/P1

> **已完成：** supportedFeatures (Task 2) 和 PartiallyInvalid (Task 3) 已实现，见 `skills/01-architecture/01-controller/10-status-management.md`。
> **不实现：** Gateway addresses (Task 4)，见 `skills/08-gateway-api/01-not-implemented.md`。
> **待定：** Programmed (Task 1)。

## 1. Programmed 条件 [P0]

### 现状

- `status_utils.rs:9-12` 注释明确说明：因 Controller-Gateway 异步 gRPC sync 架构，无数据面状态回传通道，故意省略 Programmed 和 Ready 条件
- Gateway API 规范将 `Programmed` 列为 Core 必需条件

### 建议方案

规范允许在配置**发送到**数据面后设置 `Programmed: True`（不要求数据面确认生效）。方案：

1. gRPC ConfigSync `Watch`/`List` 推送成功后，Controller 标记对应资源 `Programmed: True`
2. 若无 gateway 连接或推送失败，设置 `Programmed: False`，reason `NoConnectedDataPlane`
3. Gateway 级别和 Listener 级别均需设置

### 涉及文件

- `src/core/controller/sync_runtime/resource_processor/handlers/gateway.rs`
- `src/core/controller/sync_runtime/status_utils.rs`
- `src/core/controller/conf_sync/` — 推送结果回调

---

## ~~2. GatewayClass.status.supportedFeatures [P1]~~ ✅ 已完成

### 实现摘要

- `GatewayClassStatus` 添加 `supported_features: Option<Vec<String>>` 字段
- `gateway_class.rs` handler 的 `update_status()` 中填充 23 个已支持特性（与 conformance test 声明一致）
- 涉及文件：`src/types/resources/gateway_class.rs`、`handlers/gateway_class.rs`

---

## ~~3. PartiallyInvalid 条件 [P1]~~ ✅ 已完成

### 实现摘要

- Controller 逐 rule 验证 backend ref，结果存入 `spec.invalid_rule_indices: Option<Vec<usize>>`
- 部分 rule 无效：`Accepted: True` + `PartiallyInvalid: True`；全部 rule 无效：`Accepted: False`
- Gateway 编译路由时 skip `invalid_rule_indices` 中的 rule
- Backend ref 存在性检查从 `validate()` 移至 `update_status()`，避免误标整条路由 Accepted: False
- HTTPRoute 和 GRPCRoute 均已实现
- 集成测试：`HTTPRoute/Backend/PartiallyInvalid`（3 cases: valid rule 代理、invalid rule 跳过、全部无效拒绝）
- 详细架构见 `skills/01-architecture/01-controller/10-status-management.md`

---

## 4. Gateway addresses 状态 [P2] — 不实现

- 从 spec.addresses、Service ClusterIP、默认配置、fallback `0.0.0.0` 推导
- 不反映数据面实际绑定地址（受限于无状态回传通道）
- 若未来为 Programmed 条件建立状态回传通道，可顺带解决
- 详见 `skills/08-gateway-api/01-not-implemented.md`
