# Edgion 项目内存泄漏深度审查报告

**审查日期**: 2026-03-08
**审查范围**: src/ 目录全部核心 Rust 代码（约 534 个 .rs 文件）
**审查重点**: 内存泄漏、资源泄漏、无限增长的数据结构

---

## 总体评价

Edgion 项目的代码质量**整体较高**。Rust 的所有权系统在底层有效防止了大部分传统意义上的内存泄漏。项目广泛使用的 `ArcSwap` + `Arc` + RCU 模式总体正确，配置热更新时旧数据能通过引用计数自动回收。

但审查仍发现 **17 个潜在问题**，按严重程度分布如下：

| 严重程度 | 数量 | 说明 |
|---------|------|------|
| **高** | 2 | 可能导致持续内存增长的真实泄漏风险 |
| **中** | 7 | 特定条件下的内存/性能问题 |
| **低** | 8 | 代码质量、效率优化、防御性编程改进 |

---

## 高严重度问题概览

| # | 问题 | 模块 | 详情文件 |
|---|------|------|---------|
| H-1 | 辅助 LB Store 僵尸条目不可回收 | gateway/backends | [01-backends.md](./01-backends.md) |
| H-2 | EWMA/LeastConn 全局 DashMap 条目永不清理 | gateway/lb | [05-lb.md](./05-lb.md) |

## 中严重度问题概览

| # | 问题 | 模块 | 详情文件 |
|---|------|------|---------|
| M-1 | UDP listener JoinHandle 被丢弃 | gateway/runtime | [02-runtime.md](./02-runtime.md) |
| M-2 | ULogBuffer 无大小限制 | gateway/plugins | [03-plugins.md](./03-plugins.md) |
| M-3 | OpenidConnect 缓存粗暴清理策略 | gateway/plugins | [03-plugins.md](./03-plugins.md) |
| M-4 | HTTP Header/Query 正则每请求编译 | gateway/routes | [04-routes.md](./04-routes.md) |
| M-5 | CacheData 后台 tokio::spawn 任务无法取消 | conf_sync | [06-conf-sync.md](./06-conf-sync.md) |
| M-6 | gRPC Watch stream 后台任务无法取消 | conf_sync | [06-conf-sync.md](./06-conf-sync.md) |
| M-7 | LinkSys dispatch 后台任务 JoinHandle 丢弃 | gateway/link_sys | [07-link-sys.md](./07-link-sys.md) |

## 低严重度问题概览

| # | 问题 | 模块 | 详情文件 |
|---|------|------|---------|
| L-3 | 健康检查任务缺少优雅取消 | gateway/backends | [01-backends.md](./01-backends.md) |
| L-4 | 健康检查并发任务无上限 | gateway/backends | [01-backends.md](./01-backends.md) |
| L-6 | ExtensionRef body filter 缺少 panic 保护 | gateway/plugins | [03-plugins.md](./03-plugins.md) |
| L-7 | gRPC full_set 双重 Clone | gateway/routes | [04-routes.md](./04-routes.md) |
| L-8 | add_header 方法签名错误 | gateway/runtime | [02-runtime.md](./02-runtime.md) |

---

## 审查通过的关键模块

以下模块经过深度审查，**未发现内存泄漏问题**：

- **ArcSwap 使用模式**：全项目的 `store()` / `load()` 使用正确，旧值能通过引用计数自动回收
- **Arc 循环引用**：未发现任何 Arc 循环引用，所有引用链为单向 DAG
- **Workqueue 队列管理**：有容量限制（bounded channel）、去重（DashSet）、退避重试上限、触发链循环检测
- **EdgionHttpContext 生命周期**：per-request 创建和销毁，所有容器字段随 ctx drop 释放
- **插件间数据传递**：正确使用 `std::mem::take` 进行所有权转移
- **EventStore 循环缓冲**：有固定容量，旧事件被覆盖时正确释放
- **ClientRegistry**：有 register/unregister 完整生命周期
- **ES bulk ingest loop**：有 shutdown 信号、channel 关闭检测、数据 flush
- **Webhook Manager**：upsert 时正确 abort 旧的健康检查任务，remove 时清理完整

---

## 修复优先级建议

1. **立即修复**: H-1 (辅助 LB Store 僵尸条目)、H-2 (EWMA/LeastConn 全局 DashMap)
2. **计划修复**: M-4 (正则预编译)、M-3 (OIDC 缓存策略)
3. **改进优化**: M-1, M-2, L-1 等代码质量问题

详细的问题描述、代码定位和修复建议请参见各子文件。
