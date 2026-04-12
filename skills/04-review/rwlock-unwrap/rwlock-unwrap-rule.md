---
name: rwlock-unwrap-review-rule
description: Review 中遇到 RwLock/Mutex .unwrap() 时的判定规则：只有写临界区内可能 panic 才构成真实风险，否则是误报或过度定级。
---

# RwLock `.unwrap()` Review 判定规则

## 核心结论

**不要对所有 `.unwrap()` 报警，只看写临界区是否可能 panic。**

`std::sync::RwLock` 中毒（Poison）的唯一触发条件：**持有写锁的线程在持锁期间 panic**。  
读锁持有者 panic 不会使锁中毒。

## 判定步骤

1. 找到所有 `write().unwrap()` 的调用点
2. 查看**持有写锁期间**执行了什么操作
3. 若临界区只有 `HashMap::insert`、`HashMap::clear`、字段赋值等，这些操作**不可能 panic** → 无实际风险，误报或过度定级
4. 若临界区内有用户数据驱动的解析、`unwrap()`、`assert!` 等 → 有真实风险，值得修

## 对读锁

`read().unwrap()` 仅在锁已被写路径中毒后才会失败。如果写临界区不可能 panic（见上），这些 `unwrap` 同样无实际风险。

## 参考案例

`processor_registry.rs` 的 12 处 `unwrap` 被评为 P0，实际写临界区只有：
- `map.insert(kind, processor)` — 不会 panic
- `map.clear()` — 不会 panic

结论：当前无实际 Poison 风险，可降为 P2（防御性改进，不需要专门修复）。

---

## `std::sync` 在 async 上下文中——常见误报

**报告模式**："`std::sync::Mutex/RwLock` 在 async 函数中使用，应换 `tokio::sync`"。

**判定步骤**：

1. 检查 guard 是否跨越 `.await` —— 若临界区在 `await` 前已结束（临时变量、块作用域），**无问题**。
2. 检查 poison 风险（见上节）—— 若写临界区不可能 panic，poison 不会触发，换锁没有实质收益。
3. 若确实需要跨 await 持锁，才考虑 `tokio::sync::Mutex`；但这会使同步 trait 方法变复杂（需 `block_on`），接口往往变差。

**不要引入 `parking_lot` 的理由**：其优势是"无 poison"，但如果写临界区本就不会 panic，poison 是死代码路径，引入 `parking_lot` 只是把 `.unwrap()` 换成不需要 `.unwrap()` 的写法，无实质改善。

**Edgion controller 层结论**：conf_mgr 模块经核查，无 guard 跨 await 的违规，写临界区均为安全操作，`std::sync` 保持原样即可。
