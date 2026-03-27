# ReferenceGrant API 版本从 v1beta1 升级到 v1

## 优先级: P0
## 工作量: 小（预计 0.5 天）
## 类型: 版本升级

---

## 问题描述

Edgion 的 ReferenceGrant 仍使用 `gateway.networking.k8s.io/v1beta1` API 版本。
Gateway API 从 v1.1.0 起 ReferenceGrant 已 GA 为 v1。继续使用 v1beta1 影响一致性声明和长期兼容性。

## 当前状态

- 功能完整：source/target kinds、name 级粒度、运行时重新评估均已实现
- 仅 API 版本字符串需要更新
- v1beta1 和 v1 的 spec 字段完全相同，无结构变化

## 涉及文件

1. `src/types/resources/reference_grant.rs` — 资源类型定义，API version 字符串
2. `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs` — Handler
3. CRD YAML 文件（如有）— API version 声明
4. 集成测试 YAML — `apiVersion` 字段
5. `examples/test/conf/ReferenceGrant/` — 所有测试用例的 apiVersion

## 实现建议

1. 搜索所有 `v1beta1` 出现位置中与 ReferenceGrant 相关的引用
2. 将 API version 从 `gateway.networking.k8s.io/v1beta1` 改为 `gateway.networking.k8s.io/v1`
3. 确保 CRD 定义同时支持 v1 和 v1beta1（向后兼容）
4. 更新所有集成测试 YAML 文件
5. 运行 `cargo check --all-targets` 和集成测试验证

## 参考 Skills

- `skills/01-architecture/05-resources/SKILL.md` — 资源系统架构
- `skills/01-architecture/00-common/03-resource-system.md` — 资源注册和版本管理
- `skills/05-testing/00-integration-testing.md` — 集成测试验证

## 验证标准

- `cargo check --all-targets` 通过
- `cargo clippy --all-targets` 通过
- ReferenceGrant 相关集成测试通过
- 搜索代码中不再有 ReferenceGrant + v1beta1 的组合（CRD 向后兼容除外）
