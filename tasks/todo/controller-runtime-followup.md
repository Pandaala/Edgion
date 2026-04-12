# ControllerRuntime 重构后续事项

ControllerRuntime 主体重构已完成（feature-04-06 分支），遗留以下几个跟进项。

---

## ~~T1：ConfigMapStore 和 BackendTLSPolicy 迁移进 ControllerRuntime 字段~~ ✅ 已完成

`ConfigMapStore` → `runtime.configmap_store`，`BackendTLSPolicyTargetManager` + `POLICY_TIMESTAMPS` → `runtime.backend_tls_policy`（新建 `BackendTLSPolicyState` struct）。全局静态和自由函数已全部删除。

---

## T2：为 reset() 添加单元测试

**优先级**：P2  
**背景**：目前没有测试验证 `reset()` 的行为：所有字段清空、listener 保留、NamespaceStore/ListenerPortManager 不清。

**建议测试内容**（在 `runtime.rs` 的 `#[cfg(test)]` 模块中）：
1. `reset()` 后 `processor_registry` 为空（`count() == 0`）
2. `reset()` 后 `ref_grant_store` 为空（`get("any")` 返回 `None`）
3. `reset()` 不清 `ref_grant_dispatcher` 的 listener（listener 数量不变）
4. `reset()` 不清 `namespace_store`（已有数据仍可查询）

---

## T3：整理 KubernetesCenter 中的 reset() 双重调用

**优先级**：P3（幂等无害，纯代码整洁）  
**背景**：`run_main_flow` 结束时调用一次 `reset()`，`run_leader_only_lifecycle` 在 Shutdown/LostLeadership 分支也调用一次，导致某些路径重复 reset。  
**建议**：确认所有 `run_main_flow` 退出路径都经过外层 lifecycle 的 reset，移除内层的重复调用。

---

## 不需要处理的项（已确认设计合理）

- **Arc 循环引用**：`ControllerRuntime` → `ProcessorRegistry` → `Arc<ResourceProcessor>` → `Arc<ControllerRuntime>`，以及 dispatcher → listener → runtime。`ControllerRuntime` 是进程级对象，永不 drop，循环不导致泄漏；`clear_processors()` 在每次 restart 时打破第一条循环。已在 `runtime.rs` 注释中文档化。
- **`GLOBAL_SECRET_STORE` 保留**：供 Gateway binary 的 plugin 使用，与 Controller 层隔离，不需要迁移。注释已标注。
- **`NamespaceStore`/`ListenerPortManager` 不在 reset() 中清理**：前者由 `Event::InitDone` 自愈，后者由 Gateway reconcile 重填，已文档化。
