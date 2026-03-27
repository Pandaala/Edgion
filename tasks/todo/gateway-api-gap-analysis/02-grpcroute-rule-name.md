# GRPCRoute Rule `name` 字段缺失

## 优先级: P0
## 工作量: 小（预计 0.5 天）
## 类型: 字段缺失

---

## 问题描述

Gateway API v1.2.0+ 为所有 Route Rule 添加了可选的 `name` 字段（SectionName 类型），用于规则标识和可观测性。
HTTPRoute 已实现此字段，但 GRPCRoute 的 `GRPCRouteRule` 结构体缺少 `name` 字段。
这导致 `GRPCRouteNamedRule` 一致性测试无法通过。

## 当前状态

- HTTPRoute: ✅ 已有 `name: Option<String>` 字段
- GRPCRoute: ❌ `GRPCRouteRule` 结构体无 `name` 字段
- 规则只能通过数组索引引用，无法通过名称标识

## 涉及文件

1. `src/types/resources/grpc_route.rs` — `GRPCRouteRule` 结构体定义（约 68-112 行）
2. `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/grpc_route.rs` — 状态报告逻辑
3. `src/core/gateway/routes/grpc/match_engine.rs` — 匹配引擎（可选：日志中使用 rule name）
4. `src/core/gateway/routes/grpc/match_unit.rs` — 匹配单元编译

## 实现建议

1. 在 `GRPCRouteRule` 结构体中添加字段：
   ```rust
   #[serde(default, skip_serializing_if = "Option::is_none")]
   pub name: Option<String>,
   ```
2. 参考 HTTPRoute 中 `name` 字段的用法，保持一致
3. 在状态报告中，优先使用 rule name（如有），否则回退到数组索引
4. 在 access log / 可观测性输出中包含 rule name（如有）

## 参考 Skills

- `skills/01-architecture/05-resources/SKILL.md` — 资源字段定义规范
- `skills/03-coding/observability/00-access-log.md` — 可观测性中的规则标识
- `skills/05-testing/00-integration-testing.md` — 集成测试

## 参考实现

查看 HTTPRoute 中 `name` 字段的定义和使用方式：
- `src/types/resources/http_route.rs` — HTTPRouteRule.name 字段
- 搜索 `rule.name` 或 `rule_name` 了解 HTTPRoute 中的用法模式

## 验证标准

- `GRPCRouteRule` 结构体包含 `name: Option<String>` 字段
- 序列化/反序列化正确（带 name 和不带 name 均可）
- `cargo check --all-targets` 通过
- `cargo clippy --all-targets` 通过
