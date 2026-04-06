# HandlerContext 与全局 PROCESSOR_REGISTRY 紧耦合：可测试性审查

## 问题概述

`HandlerContext` 在 `secret_ref_manager`、`metadata_filter`、`namespace_filter` 等字段上采用**构造期注入**，与 `ResourceProcessor::create_context` 的设计一致；但 `requeue` 路径在实现上直接调用 `crate::core::controller::conf_mgr::PROCESSOR_REGISTRY` 的 `requeue_with_chain`，形成**「半注入、半全局」**的混合模型。

该设计带来三类后果：

1. **单元测试无法在无全局环境下断言 requeue 行为**（除非 mock 进程级单例或接受集成测试粒度）。
2. **Handler 开发者**在编写针对 `on_change` / `parse` 等会间接触发级联重排的测试时，被迫与真实 Registry、Workqueue、延迟合并子系统耦合。
3. **模块依赖方向**：`sync_runtime::resource_processor::context` 向上依赖 `conf_mgr` 中的静态 `PROCESSOR_REGISTRY`，而 `processor_registry.rs` 又向下引用 `resource_processor` 中的 `ProcessorObj` 与若干全局 getter，构成** crate 内逻辑上的双向依赖**（虽可通过 Rust 的模块图编译，但增加理解与拆分边界的成本）。

**相关代码**（摘录自 `context.rs`）：

```91:118:Edgion/src/core/controller/conf_mgr/sync_runtime/resource_processor/context.rs
    pub async fn requeue(&self, kind: &str, key: String) {
        if self.trigger_chain.depth() >= self.max_trigger_depth {
            // ...
            return;
        }
        if self
            .trigger_chain
            .would_exceed_cycle_limit(kind, &key, self.max_trigger_cycles)
        {
            // ...
            return;
        }
        PROCESSOR_REGISTRY
            .requeue_with_chain(kind, key, self.trigger_chain.clone())
            .await;
    }
```

`ProcessorRegistry::requeue_with_chain` 的职责（与 `HandlerContext` 分工）：

- `HandlerContext::requeue`：深度 / 环检测（`max_trigger_depth`、`max_trigger_cycles` + `TriggerChain`）。
- `ProcessorRegistry::requeue_with_chain`：`get(kind)` 后调用 `processor.requeue_with_chain`（含延迟子系统上的合并语义）。

---

## 1. 当前测试覆盖的限制分析

### 1.1 无法隔离「重排队列」副作用

对 `ctx.requeue(kind, key)` 的断言在理想单元测试中应能验证：

- 在给定 `TriggerChain` 下，**是否**向某 `(kind, key)` 发起了调度请求；
- 在超过深度或环限制时，**是否**未发起调度。

当前实现下，最后一步固定落到**全局** `PROCESSOR_REGISTRY`。测试若要观察行为，只能选择：

| 手段 | 代价 / 风险 |
|------|-------------|
| 向真实 `PROCESSOR_REGISTRY` 注册带 spy 的 `ProcessorObj` mock | 需完整启动/注册路径；多测试并行时与 `clear_registry`、其他全局存储（`clear_registry` 内联动的 `ServiceRefManager`、`GatewayRouteIndex` 等）纠缠；与 [`01-global-singletons.md`](./01-global-singletons.md) 中描述的进程级状态泄漏问题同构。 |
| 仅测「环检测 / 深度」之前的逻辑 | 无法在不碰全局的情况下证明「通过后确实调用了 registry」；只能做日志/集成级黑盒。 |
| 集成 / E2E 测试 | 成本高、 flakiness 与环境依赖大，不适合作为 handler 回归的主力。 |

### 1.2 与仓库内现有测试模式的对比

仓库中部分 handler 测试（如 `grpc_route.rs`、`edgion_tls.rs`、`edgion_plugins.rs`）通过 `HandlerContext::new(...)` 构造上下文，但**未覆盖** `requeue` 路径的替身注入；一旦测试调用 `ctx.requeue`，行为仍绑定全局 Registry。

`processor_registry.rs` 内仅存在对**独立** `ProcessorRegistry::new()` 的基础测试（空 registry `is_all_ready` 等），**并未**与 `HandlerContext` 组合成「可替换 requeue 后端」的测试夹具。

### 1.3 调用面规模（requeue 经 Context）

当前经 `HandlerContext::requeue` 触发的典型路径包括（非穷尽）：

- `handlers/gateway.rs`、`configmap.rs`、`service.rs`、`secret.rs` 中的级联重排；
- `handlers/mod.rs` 中 `requeue_parent_gateways`（各 Route handler 经此间接触发 Gateway requeue）。

此外，大量 handler 仍直接 `PROCESSOR_REGISTRY.get(...)` 做跨 Kind 读路径（如 `gateway.rs`、`route_utils.rs`），与本文主题「Context 与 Registry」叠加后，**单测隔离难度进一步放大**（即使完成 Context 注入，lookup 仍可能走全局）。

---

## 2. 对 Handler 开发者的影响

### 2.1 单元测试应如何写（现状下的务实做法）

在**不改动生产代码**的前提下，Handler 开发者大致只能：

1. **避免在单测中执行 `ctx.requeue`**：将「应 requeue 哪些 (kind, key)」抽成纯函数或小型结构，仅对纯逻辑做单测；`on_change` 里保留对 `ctx.requeue` 的薄封装——这会增加分层与样板代码，且无法保证薄封装本身无错。
2. **依赖集成测试**：启动或模拟接近真实的 Controller 启动顺序，注册 processors，再观察 workqueue / 日志——调试周期长。
3. **全局 setup/teardown**：在 `#[serial]` 或单线程测试中 `register` mock processor、`clear_registry` 清理——易与并行测试冲突，且 `clear_registry` 会清空多种全局索引，副作用面大。

### 2.2 与 `SecretRefManager` 注入模式的不一致

`ResourceProcessor::create_context` 将 `Arc<SecretRefManager>` 注入 `HandlerContext`，使 Secret 依赖追踪**可按处理器实例**在测试中替换或共享同一 `Arc`。`requeue` 若继续走静态 Registry，则**同一上下文对象**在依赖模型上呈现「有的可注入、有的不可注入」，新成员容易误用全局 `PROCESSOR_REGISTRY` 与 Context API 混写（与代码库中已存在的 `PROCESSOR_REGISTRY.get` 直调并存）。

### 2.3 API 契约层面的隐含假设

`ProcessorHandler` 的方法签名仅暴露 `&HandlerContext`，**不**暴露 Registry；但 `requeue` 的实际语义是「向**进程内唯一** Registry 发请求」。这在文档与类型上均未显式编码，依赖阅读实现或运维文档（如 skills 中的 requeue 机制说明）。

---

## 3. 注入方案设计

目标：让 `HandlerContext::requeue` 的**最后一步**变为对「可替换后端」的调用，同时尽量保留现有环检测逻辑于 Context 内（单一职责：Context 持有 chain 与阈值，后端负责 `get(kind)` + `requeue_with_chain`）。

### 3.1 方案 A：`Arc<ProcessorRegistry>` 注入（最小抽象）

在 `HandlerContext` 中增加字段，例如 `requeue_target: Arc<ProcessorRegistry>`（命名可按团队惯例调整为 `registry`），`requeue` 改为调用 `self.requeue_target.requeue_with_chain(...)`。

- **优点**：改动面相对小；生产路径传入 `Arc::clone(&*LazyLock::...)` 或与静态 `PROCESSOR_REGISTRY` 包装为 `Arc`（若将 `ProcessorRegistry` 改为可由 `Arc` 共享的同一份实例）；测试中构造独立 `ProcessorRegistry::new()` 并注册 mock `ProcessorObj`。
- **缺点**：`ProcessorRegistry` 仍是具体类型，若测试需完全假实现（不建 HashMap、不持锁），仍需注册真实 shaped 的 mock processor；**未**打破 `resource_processor` 对 `ProcessorRegistry` 类型的编译依赖（可考虑把 `ProcessorRegistry` 迁到更小模块以减少「概念层」耦合）。

### 3.2 方案 B：Trait 对象 `Arc<dyn RequeueDispatcher>`（推荐用于强可测性）

定义窄 trait，例如：

```rust
#[async_trait::async_trait]
pub trait RequeueDispatcher: Send + Sync {
    async fn requeue_with_chain(&self, kind: &str, key: String, chain: TriggerChain);
}
```

为 `ProcessorRegistry` 实现该 trait（委托现有方法）。`HandlerContext` 持有 `Arc<dyn RequeueDispatcher>`。

- **优点**：单测可使用 `Arc::new(MockDispatcher { recorded: Mutex::new(Vec::new()) })` 记录调用，无需真实 workqueue；并行测试互不干扰。
- **缺点**：引入 `async_trait` 或手动 `Pin<Box<dyn Future>>` 的权衡；`dyn` 调用一次间接开销（在 requeue 频率下通常可忽略）。

### 3.3 方案 C：闭包 / 函数指针（仅当希望零额外 trait 时）

例如 `Arc<dyn Fn(&str, String, TriggerChain) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>`，在 Rust 中**人体工程学差**，一般不推荐；若坚持，可封装为具体 `struct RequeueFn(...)`。

### 3.4 与模块依赖、循环依赖的关系

- **短期**：即使采用方案 A/B，`processor_registry.rs` 仍可能依赖 `resource_processor` 中的 `ProcessorObj`；但 **`context.rs` 可去掉对 `PROCESSOR_REGISTRY` 静态符号的引用**，改为仅依赖「trait 或 `ProcessorRegistry` 类型」——若将 `RequeueDispatcher` trait 定义在 `sync_runtime::workqueue` 或 `resource_processor` 下的 `dispatch.rs` 等**双方公共子模块**，由 `conf_mgr::processor_registry` 实现，则**可消除 `context → static PROCESSOR_REGISTRY` 这条边**，循环依赖在「静态依赖图」上减弱。
- **长期**：若进一步将 `ProcessorRegistry` 拆到独立 crate 或 `conf_mgr` 子模块，并令 `resource_processor` 只依赖 trait，架构边界会更清晰（工作量大，非本变更必需）。

### 3.5 构造 API 建议

- `HandlerContext::new` 增加参数 `requeue: Arc<dyn RequeueDispatcher>`（或 `Arc<ProcessorRegistry>`）。
- 可选：`HandlerContext::new_for_test(...)` 或 `#[cfg(test)]` 构造函数，减少生产调用点噪音（注意不要在 lib 中滥用 `cfg(test)` 暴露仅测试 API，除非团队接受）。
- `ResourceProcessor::create_context`：持有 `Arc<dyn RequeueDispatcher>` 克隆（来源可为 `ResourceProcessor` 构造时注入，默认在 `kubernetes/controller` / `file_system/controller` 中绑定全局单例的包装）。

---

## 4. 向后兼容的迁移路径

建议分阶段，避免大爆炸式重构。

### 阶段 0：文档与约束

- 在 `HandlerContext` 模块文档中明确：`requeue` 当前依赖进程级 Registry；新增 handler 若需单测级联行为，优先抽纯函数或使用集成测试（直至阶段 2 完成）。

### 阶段 1：引入注入字段，默认绑定全局

1. 定义 `RequeueDispatcher`（若选方案 B）或确认 `Arc<ProcessorRegistry>` 来源。
2. `HandlerContext` 增加字段；`requeue` 使用字段而非直接 `PROCESSOR_REGISTRY`。
3. `ResourceProcessor` 增加可选依赖：默认用 `LazyLock` 提供的全局 registry 的 `Arc` 包装。若当前 `ProcessorRegistry` 不是 `Arc` 共享，需引入 **`Arc<ProcessorRegistry>` 与静态 `LazyLock<Arc<ProcessorRegistry>>`** 或 **单例包装 struct**，使全局与注入指向同一实例（避免双份 registry）。
4. 全量替换 `HandlerContext::new` 调用点（`processor.rs::create_context`、各测试 `make_ctx`）。

此阶段**对外行为不变**，仅挪动调用边。

### 阶段 2：测试与 Mock

- 为 `RequeueDispatcher` 提供测试用 `RecordingDispatcher`。
- 择 1～2 个高价值 handler（如 `service.rs` 中 `requeue_dependent_routes`）补充单测，验证在伪造 chain 下调用参数正确、环检测仍生效。

### 阶段 3（可选）：收敛直调 `PROCESSOR_REGISTRY`

- 将仅用于「读 processor / requeue」的 handler 路径逐步改为经 Context 或经显式注入的 `Arc<ProcessorRegistry>` / `dyn RequeueDispatcher`，减少全局 `get` 散落（与本文相关但范围更大，可单独立项）。

### 风险与回滚

- **风险**：`Arc` 化全局 `ProcessorRegistry` 时，需审计所有 `PROCESSOR_REGISTRY` 使用点是否应指向同一 `Arc`；`clear_registry` 等行为必须仍作用在该唯一实例上。
- **回滚**：保留 `requeue` 实现为「若未注入则回退静态」的过渡 API 会增加技术债，**不推荐**；更稳妥是阶段 1 一次性打通所有构造路径。

---

## 5. 结论与优先级建议

| 维度 | 结论 |
|------|------|
| **根因** | `HandlerContext::requeue` 硬编码全局 `PROCESSOR_REGISTRY`，与 Context 其余注入字段不一致，且无法在进程内替换。 |
| **测试** | 级联 requeue 行为难以做快速、并行、确定性的单元测试；与全局单例测试问题叠加。 |
| **推荐方向** | 优先 **方案 B（`Arc<dyn RequeueDispatcher>`）** 或 **方案 A（`Arc<ProcessorRegistry>`）** 注入，在 `ResourceProcessor::create_context` 统一传入；并规划将 trait 置于公共子模块以弱化 `context.rs` 对静态符号的依赖。 |
| **优先级** | 与 [`01-global-singletons.md`](./01-global-singletons.md) 中 P1「HandlerContext 注入 registry」一致；建议在锁 poison / 异步持锁等 P0 项稳定后推进，或与「测试基建」专项合并。 |

---

## 参考路径

- `src/core/controller/conf_mgr/sync_runtime/resource_processor/context.rs` — `HandlerContext`、`requeue`
- `src/core/controller/conf_mgr/processor_registry.rs` — `PROCESSOR_REGISTRY`、`requeue_with_chain`
- `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs` — `create_context`
- `tasks/review/conf_center/01-global-singletons.md` — 全局单例与可测试性总览
