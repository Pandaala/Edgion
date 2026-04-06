# Conf Center 全局可变单例使用审查

## 问题概述

Controller 侧配置管理（`conf_mgr`）及相关资源处理器模块中，大量使用 `std::sync::LazyLock` / `OnceLock` 暴露进程级全局状态，并通过 `get_global_*` 风格的自由函数在任意调用点读取。`ProcessorRegistry::clear_registry()` 承担「统一清理」职责时，需要**手工枚举**各全局存储的 `clear` / `replace_all` 调用，与新增全局状态之间没有编译期约束，容易出现**清理遗漏**或与业务语义不一致（部分存储有意不清理，部分则属于疏漏）。

同时，`HandlerContext` 虽对 `SecretRefManager` 采用**按处理器注入**的 `Arc`，但跨资源 `requeue` 仍直接依赖全局 `PROCESSOR_REGISTRY`，形成「半注入、半全局」的混合架构，增加理解与演进成本。

**审查范围说明**：本文聚焦 **Conf Center / `resource_processor` 周边** 与用户列出的 9 类全局状态；Gateway 运行时、指标、配置 `OnceLock` 等虽同样存在全局模式，仅在与 Controller 清理/测试隔离相关处点到为止。

---

## 现状分析

### 1. 组件一览与访问模式

| 符号 / Getter | 实现 | 主要职责 | 典型访问方式 |
|---------------|------|----------|----------------|
| `PROCESSOR_REGISTRY` | `LazyLock<ProcessorRegistry>` | 按 Kind 注册 `ResourceProcessor`、跨资源 `requeue`、`all_watch_objs`、就绪门控 | 多处 `use ...::PROCESSOR_REGISTRY`；`HandlerContext::requeue` 直接调用 |
| `SecretStore` | `LazyLock<Arc<SecretStore>>` | 全局 Secret 快照，供 TLS 回调等跨模块只读 | `get_global_secret_store` / `get_secret` / `replace_all_secrets` |
| `ServiceRefManager` | `LazyLock<ServiceRefManager>`（实为 `BidirectionalRefManager` 别名） | Service ↔ Route 反向引用，Service 变更时重排队列 | `get_service_ref_manager()`；`clear_registry` 中 `clear()` |
| `CrossNamespaceRefManager` | `OnceLock<Arc<...>>` | 跨命名空间引用与重验 | `get_global_cross_ns_ref_manager()`；路由/Gateway 等 handler |
| `ReferenceGrantStore` | `OnceLock<Arc<ReferenceGrantStore>>` | ReferenceGrant 索引与 `check_reference_allowed` | `get_global_reference_grant_store()` |
| `GatewayRouteIndex` | `LazyLock<GatewayRouteIndex>` | Gateway ↔ Route 反向索引，监听器 hostname/port 变更触发重排队列 | `get_gateway_route_index()` |
| `AttachedRouteTracker` | `LazyLock<AttachedRouteTracker>` | 附加路由计数/附件关系，Gateway status 使用 | `get_attached_route_tracker()` |
| `ListenerPortManager` | `LazyLock<ListenerPortManager>` | 单 Gateway 内端口/hostname 冲突检测 | `get_listener_port_manager()`（如 `gateway` handler） |
| `NamespaceStore` | `LazyLock<NamespaceStore>` | Namespace 标签缓存，selector 策略与 `route_utils` | `get_namespace_store()` |

**与「SecretRefManager」的区分（易混淆点）**：

- **Secret → 依赖资源的反向索引**：每个 `ResourceProcessor` 在构造时使用 **`Arc::new(SecretRefManager::new())`**，经 `HandlerContext` 注入，**不是**上述表格中的全局单例。
- **Service → Route**：使用 **全局** `get_service_ref_manager()`，与 Secret 侧「每处理器一份」不对称。

### 2. `clear_registry()` 实际清理范围

`processor_registry.rs` 中 `clear_registry` 当前顺序大致为：

1. 清空 `ProcessorRegistry` 内 `HashMap`（注销所有 processor）
2. `get_service_ref_manager().clear()`
3. `get_global_reference_grant_store().replace_all(empty)`
4. `get_global_cross_ns_ref_manager().clear()`
5. `get_gateway_route_index().clear()`
6. `get_attached_route_tracker().clear()`
7. `replace_all_secrets(empty)`（清空全局 SecretStore）

**未包含**（与注释或代码一致或需单独核实业务意图）：

- **`ListenerPortManager`**：注释写明已从统一清理中移除（冲突检测改为仅 Gateway 内语义）；**`clear_registry` 不会**清空端口管理状态。
- **`NamespaceStore`**：代码路径上**未见**在 `clear_registry` 中 `replace_all` 或等价清空；若 Leader 切换或全量重启依赖「仅 processor + 部分全局」重置，可能出现 **Namespace 标签视图与 processor 空窗期不一致**（直至 Namespace informer 再次同步）。
- **`ReferenceGrantEventDispatcher`（`get_global_dispatcher`）**：`OnceLock` 持有监听器列表；**不在** `clear_registry` 枚举中，若 listener 持有旧 processor 时代闭包/引用，存在生命周期与重复注册方面的隐患（需结合注册点审计）。

### 3. 访问模式归纳

- **Handler 层**：大量 handler 直接 `get_global_*()`，与 `PROCESSOR_REGISTRY.get(...)` 混用，调用链难以从函数签名推断依赖。
- **上下文对象**：`HandlerContext` 携带 `secret_ref_manager`（注入），但 `requeue` 固定走全局 `PROCESSOR_REGISTRY.requeue_with_chain`（见 `context.rs`）。
- **Re-export 面**：`resource_processor/mod.rs` 集中 re-export 各 `get_global_*`，进一步鼓励「随处可 import」的用法。

---

## 风险评估

### 1. 可测试性与进程内隔离

- 全局 `LazyLock`/`OnceLock` 在**同一进程多测试并行**时，状态在测试间泄漏；常见缓解手段是「串行测试 + 手工 reset」或 `OnceLock` 无法重置导致的**不可逆初始化**（例如 `ReferenceGrantStore` / `CrossNsRefManager` 的 `Arc` 单例一旦 `get_or_init` 后，仅能通过类型自身 API 清空内部 map，无法替换整个 `Arc` 实例）。
- `ProcessorRegistry` 与多个全局存储**生命周期绑定整个进程**，难以构造「仅替换某一子系统」的单元测试替身。

### 2. 并发与一致性

- 各存储内部多使用 `RwLock`/`Mutex`，**单结构体层面**通常线程安全；风险主要在于**跨多个全局存储的业务不变量**：例如先 `clear_registry` 清空 processor，但 `ListenerPortManager` 仍保留旧 Gateway 注册，直至新 Leader 重新 reconcile——若中间有读路径依赖「processor 与端口状态一致」，可能出现短暂不一致窗口。
- `PROCESSOR_REGISTRY` 使用 `RwLock<HashMap<...>>`，高并发只读 `get` 可接受；跨资源 `requeue` 与 `clear_registry` 写锁竞争在 leadership 抖动时可能放大延迟（需结合生产指标验证，属运维级风险）。

### 3. 初始化顺序

- `LazyLock::new` 在**首次访问**时初始化，顺序由运行时调用图决定，而非由单一 `main` 或 `bootstrap` 显式声明。
- 若某路径在 processor 注册完成前访问 `PROCESSOR_REGISTRY.get`，会得到 `None`（已有 warn 日志）；**更难发现**的是：两个全局存储之间谁先填充、谁依赖谁，仅靠代码阅读，缺少「启动阶段有向无环」的保证。

### 4. 清理遗漏与演进成本

- **新增**全局缓存或索引时，没有类型系统强制要求实现 `ClearOnRegistryReset` 之类 trait，`clear_registry` **极易漏改**。
- 已观察到的「有意不清理」与「未清理」并存（`ListenerPortManager` vs `NamespaceStore`），若没有文档化不变量，后续维护者容易误判或复制错误模式。

### 5. 架构耦合

- `ProcessorRegistry` 作为「注册表」却直接调用 `get_service_ref_manager`、`get_global_reference_grant_store` 等，违反单一职责，形成 **Registry ↔ 多个全局模块** 的硬耦合，不利于将来拆进程或嵌入多实例 Controller（例如测试 harness、sidecar 模式）。

---

## 改进建议

### 1. 目标形态：显式「Controller 运行时上下文」

引入单一 **owned 或 `Arc` 共享** 的聚合结构（名称示例：`ConfControllerRuntime` / `ResourceProcessorRuntime`），包含：

- `ProcessorRegistry`（不必再 static，可为 `Arc<ProcessorRegistry>` 或内联同职责字段）
- `Arc<SecretStore>`、`ServiceRefManager`（或统一 `RefGraph` 子模块）
- `Arc<ReferenceGrantStore>`、`Arc<CrossNamespaceRefManager>`
- `GatewayRouteIndex`、`AttachedRouteTracker`
- `ListenerPortManager`、`NamespaceStore`
- （可选）`Arc<ReferenceGrantEventDispatcher>`

在 **Kubernetes / FS Controller 启动** 时构造该结构，向下传递给：

- 各 `ResourceProcessor::new` / `HandlerContext::new`（扩展字段：`registry: Arc<ProcessorRegistry>` 用于 `requeue`）
- 需要「热路径全局读」的模块可通过 **`Arc` clone** 传递，而非 `static`。

收益：依赖关系出现在 **构造函数签名** 上，便于单测注入 fake、便于文档化初始化顺序。

### 2. 清理策略：单一 `reset_for_leader_change()`（或 Drop 语义）

- 将当前 `clear_registry` 内散落的 `get_*().clear()` **收敛为运行时上下文上的一个方法**，内部按**固定顺序**调用各子模块 `clear`（必要时用 `tracing::instrument` 打阶段日志）。
- 对 **刻意不清理** 的模块（如 `ListenerPortManager`），以**注释 + 单元测试断言**说明：「Leader 切换后依赖全量 reconcile 重注册」或「必须与 Namespace informer 对齐」。
- 若 `NamespaceStore` 应在 leader 切换时清空，应**显式**纳入上述 `reset`；若不应清空，应在模块级文档说明原因，避免被误认为遗漏。

### 3. 过渡：`ArcSwap` / 可替换句柄（可选）

对于短期内无法拆掉所有 static 的场景，可考虑将「全局」收窄为 **单个 `OnceLock<Arc<Runtime>>`**，通过 `ArcSwap::store` 在测试或重启时替换整棵上下文树（需注意并发读者与 `Drop` 顺序）。这比分散的 9+ 个 static 更易推理。

### 4. 与现有良好实践对齐

- **SecretRefManager 按处理器注入** 已是正确方向；建议 **`requeue` 同样注入 `Arc<ProcessorRegistry>`**（或 `trait RequeueTarget`），消除 `context.rs` 对 `PROCESSOR_REGISTRY` 的直接引用。
- **ReferenceGrant** 相关：`get_global_dispatcher` 的监听器列表建议在 runtime `reset` 时 **clear listeners** 或 **使用弱引用/显式 unregister**，避免旧 graph 悬挂。

---

## 迁移计划

建议分阶段降低风险，优先解决「清理完整性」与「测试隔离」，再推进大规模签名改动。

### 阶段 A — 低风险加固（1～2 迭代）

1. **文档化** `clear_registry` 与未纳入模块的不变量；在 `processor_registry.rs` 顶部用注释列表维护「清理清单」，Code Review checklist 对齐。
2. **审计** `get_global_dispatcher` 注册点：Leader 切换或 `clear_registry` 后是否重复 `register_listener`、是否持有已销毁对象。
3. 为 `clear_registry` 增加 **集成测试**（单进程顺序执行）：注册 fake processor、写入各全局存储、调用 `clear_registry`、断言各存储为空或符合预期；**特别断言** `NamespaceStore` / `ListenerPortManager` 的**有意行为**。

### 阶段 B — 引入运行时聚合，静态逐步收口（中等改动）

1. 定义 `ConfControllerRuntime`，在 `kubernetes/controller.rs` / `file_system/controller.rs` **构造并持有** `Arc<ConfControllerRuntime>`。
2. 将 `HandlerContext::requeue` 改为使用注入的 `registry: Arc<ProcessorRegistry>`（或封装 `RequeueService`）。
3. 新代码禁止新增 `static` 全局；旧模块通过 `runtime` 字段访问，**static getter 标记 `deprecated`** 或仅在 mod 内保留薄封装。

### 阶段 C — 移除分散 `LazyLock`/`OnceLock`（大改动）

1. `SecretStore`、`ReferenceGrantStore`、`CrossNamespaceRefManager` 等改为 **仅由 `ConfControllerRuntime` 持有**；删除 `get_global_*` 公共 API 或缩小为 `pub(crate)` 过渡层。
2. 全量跑 controller 集成测试与 soak；关注 **Leader 选举**、**进程重启**、**双写窗口** 下行为是否与阶段 A 文档一致。

### 优先级建议

| 优先级 | 项 | 理由 |
|--------|----|------|
| P0 | `clear_registry` 与 `NamespaceStore` / `ListenerPortManager` / `dispatcher` 语义对齐 + 测试 | 直接关联正确性与故障恢复 |
| P1 | `HandlerContext` 注入 `ProcessorRegistry`，去掉 `context.rs` 对 static 的依赖 | 改动面可控，示范依赖注入 |
| P2 | `ConfControllerRuntime` 聚合 + 新功能禁止新 static | 长期可维护性 |
| P3 | 删除所有 `get_global_*` 对外 re-export | 需全仓编译与调用点迁移，适合在 B/C 交界做 |

---

## 参考代码位置（便于对照）

- `processor_registry.rs`：`PROCESSOR_REGISTRY`、`clear_registry` 实现
- `resource_processor/mod.rs`：全局 getter 的 re-export
- `resource_processor/context.rs`：`HandlerContext::requeue` 与 `PROCESSOR_REGISTRY`
- `secret_utils/secret_store.rs`：`GLOBAL_SECRET_STORE`
- `service_ref.rs`：全局 `ServiceRefManager`
- `ref_grant/store.rs`、`cross_ns_ref_manager.rs`：`OnceLock` 单例
- `ref_grant/events.rs`：`GLOBAL_DISPATCHER`
- `gateway_route_index.rs`、`attached_route_tracker.rs`、`listener_port_manager.rs`、`namespace_store.rs`：各自 `LazyLock` 与 getter

---

*文档版本：与仓库当前 `Edgion` 源码结构对应；若后续重构了 `clear_registry` 或新增全局状态，请同步更新本节清单与测试断言。*
