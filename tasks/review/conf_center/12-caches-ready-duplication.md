# Caches Ready 逻辑重复与超时硬编码 — 架构评审

## 1. 背景与问题陈述

`KubernetesCenter` 在 **leader-only**（`start_event_watchers`）与 **all-serve**（`start_event_watchers_all_serve`）两条路径中，各自 `spawn` 了一个 **caches ready watcher** 任务；`FileSystemCenter` 则在主生命周期中通过 `wait_registry_ready` 完成相近目标。三处共享「等待 `PROCESSOR_REGISTRY` 达到可对外服务状态」这一职责，但实现分叉、魔数分散，且 **就绪判定语义并不完全一致**。

本文档对应源码位置：

| 位置 | 文件 | 大致行号 |
|------|------|----------|
| K8s leader-only caches 任务 | `src/core/controller/conf_mgr/conf_center/kubernetes/center.rs` | 517–583 |
| K8s all-serve caches 任务 | 同上 | 860–908 |
| FileSystem 等待 | `src/core/controller/conf_mgr/conf_center/file_system/center.rs` | 128–168（调用 342） |

与更广义的「事件监视器重复」分析可对照：[03-event-watcher-duplication.md](./03-event-watcher-duplication.md)（其中 1.5 节已点名 caches 块重复）。本文聚焦 **就绪等待、超时、轮询、revalidation 与 ACME 边界**。

---

## 2. 三处逻辑精确对比

### 2.1 共同点

1. **超时量级**：均为 **30 秒**（K8s 为 `const CACHE_READY_TIMEOUT_SECS: u64 = 30`；FileSystem 为 `wait_registry_ready(30)`）。
2. **目标**：在对外注册 `ConfigSyncServer` / 发送 `CachesReady` 之前，尽量等到处理器侧缓存初始化完成。
3. **K8s 两条路径**：`no_sync_kinds`、`expected_sync_kinds` 的计算方式一致；`pending_sync_kinds` 闭包、200ms 轮询、`register_all`、三条 `trigger_*_revalidation` 调用 **字节级重复**（all-serve 仅省略 ACME）。
4. **FileSystem 与 K8s**：超时后均 **不崩溃**，继续后续流程（K8s 发 `CachesTimeout`；FileSystem 打 warn 后 `return` 并继续 `register_all` 等）。

### 2.2 差异矩阵

| 维度 | K8s leader-only | K8s all-serve | FileSystem |
|------|-----------------|---------------|------------|
| **就绪判定** | 先检查 `expected_processor_kinds` 是否均已 **注册**；再对 `expected_sync_kinds`（排除 `no_sync_kinds`）检查 `is_ready()` | 同左 | `PROCESSOR_REGISTRY.is_all_ready()`：注册表 **非空** 且 **当前已注册** 的 processor **全部** `is_ready()` |
| **轮询间隔** | 200ms | 200ms | **1s** |
| **成功后 `register_all` 位置** | 在 **同一 spawn 块内**、revalidation 之后 | 同左 | 在 `wait_registry_ready` **返回后**，由 `start()` 中后续步骤执行 |
| **三条 revalidation** | spawn 内 **同步顺序** 调用 | 同左 | `start()` 中 **单独步骤**（342 行之后），与 K8s **顺序一致** |
| **ACME** | 就绪后 `start_acme_service(acme_client)` | **不** 在此启动（改由 `start_leader_services`） | 无 |
| **生命周期信号** | `CachesReady` / `CachesTimeout` | 同左 | 无 channel 事件，仅本地等待 |
| **超时日志** | `warn!` 含 `timeout_secs`、`not_ready` | `warn!` 含 `ha_mode = "all-serve"`，**缺少 `timeout_secs`**（与 03 文档一致） | `warn!` 含 `timeout_secs`、`not_ready` |

### 2.3 差异原因（架构层面）

1. **ACME**：all-serve 下多副本可能同时就绪，证书服务应 **仅 leader 启停**，故从 caches 任务中拆出，集中在 `start_leader_services` / `stop_leader_services`，避免与 leader-only 单路径语义冲突。
2. **就绪判定（K8s vs FileSystem）**：K8s 控制器显式持有 **`expected_processor_kinds`**，等待「**应存在的 kind 均已注册** + **需同步的子集已 ready**」，避免在部分 processor 尚未 `register` 时误判。FileSystem 使用 **`is_all_ready()`**，等价于「**已注册集合** 全 ready」，**不** 显式对照 controller 的期望 kind 列表；在 `FileSystemController::run` 会注册固定集合的前提下通常等价，但若注册顺序或子集与 K8s 分叉，**语义上弱于 K8s**。合并公共函数时要么 **统一为「期望 kind + sync 子集」模型**，要么 **在文档与测试中固定 FileSystem 与 K8s 的契约**。
3. **轮询 200ms vs 1s**：代码库中 **无注释** 说明。可能考量包括：K8s 侧希望尽快发布 `ConfigSyncServer`、缩短就绪尾延迟；FileSystem 本地 I/O 与单进程场景下降低 wake 频率与 debug 日志噪声。**在统一配置时，应把「为何不同」写进模块级文档或默认值说明**，否则运维会误以为可随意对齐而忽略行为变化。
4. **revalidation 放在 spawn 内 vs `start()`**：K8s 将 **注册 WatchObj + revalidation** 绑在「caches 就绪」同一任务，保证事件顺序紧凑；FileSystem 将 **等待** 与 **revalidation / register_all** 拆在 `start()` 顺序步骤中，便于阅读，但 **与 K8s 结构不一致**，增加「改一处漏一处」的概率（当前已通过复制三行 `trigger_*` 保持行为一致）。

---

## 3. 提取公共「caches-ready 等待」函数的设计

### 3.1 建议职责切分

将「**纯等待 + 产出结果**」与「**副作用（register、revalidation、ACME、发事件）**」拆开，便于单测与复用。

**阶段 A — 等待（可 async 纯逻辑 + 可注入 clock/sleep 便于测试）**

- **输入**：
  - `expected_processor_kinds: &[&'static str]`（FileSystem 可从 `FileSystemController` 或静态列表传入，与 K8s 对齐）
  - `no_sync_kinds` 或已计算好的 `expected_sync_kinds`
  - `timeout: Duration`、`poll_interval: Duration`
- **输出**（建议 `enum`）：
  - `Ready` — 无 pending
  - `Timeout { not_ready: Vec<String> }` — 超时瞬间的 pending 列表（与当前 `pending_sync_kinds` 语义一致）

**阶段 B — 就绪后副作用（由调用方或小型 `CachesReadyActions` 组合）**

- `register_config_sync(css, no_sync_refs)`（K8s spawn 内；FileSystem 在 `start()` 已有 `Arc::new`，可传闭包或 trait object）
- `run_standard_k8s_revalidations().await`（三条 `trigger_*` 合一，见第 5 节）
- `Option<AcmeStart(Client)>` 或 `async fn on_ready_leader_only(...)` 仅在 leader-only 路径注入

这样 **K8s leader-only / all-serve** 共享 **阶段 A + `run_standard_k8s_revalidations`**，仅 **ACME 与 `LifecycleEvent` 发送** 分叉。

### 3.2 放置位置

- 推荐：`conf_center` 下新模块，例如 `processor_registry_ready.rs`，或挂在现有 `processor_registry` 旁 **仅 conf_center 可见** 的 `wait` 子模块，避免 `gateway` 与 `conf_mgr` 循环依赖。
- `pending_sync_kinds` 闭包改为 **自由函数** `fn pending_sync_kinds(expected_processor_kinds, expected_sync_kinds) -> Vec<&'static str>`，内部读 `PROCESSOR_REGISTRY`（与现况一致），便于单元测试时用 **可注入 Registry trait** 进阶重构（第二期）。

### 3.3 FileSystem 接入方式

- **最小改动**：`wait_registry_ready` 改为对 **共享等待函数** 的薄封装，仍暴露 `timeout_secs`；就绪判定改为与 K8s 相同的 **expected + sync 子集** 逻辑（需从 controller 暴露 `all_processor_kinds()` 或等价 API）。
- **若短期不改语义**：仅提取 **轮询 + 超时 + 日志结构**，保留 `is_all_ready()`，但在文档中明确 **与 K8s 判定差异** 及风险边界。

---

## 4. 配置化超时与 Poll 间隔

### 4.1 现状

- K8s：`CACHE_READY_TIMEOUT_SECS` 在两个 `spawn` 块内 **各自** `const`。
- FileSystem：`wait_registry_ready(30)`，调用处 **魔数 30**。
- 无任何 **结构体配置字段** 或环境变量覆盖（检索 `processor_registry`、`cache_ready` 等未见统一项）。

### 4.2 建议配置形状

在 **已有 controller / gateway 全局配置**（与 `get_no_sync_kinds()` 同级或同文件）中增加可选字段，例如：

| 字段 | 类型 | 建议默认值 | 说明 |
|------|------|------------|------|
| `processor_registry_ready_timeout_secs` | `u64` | `30` | 三处统一读取 |
| `processor_registry_ready_poll_interval_ms` | `u64` | K8s `200`，FileSystem `1000` | 允许分模式覆盖或单一全局值 |

**部署语义**：大集群 list/watch 首屏慢时，仅调 **timeout** 往往足够；**poll** 过短会增加定时器与锁竞争（`PROCESSOR_REGISTRY` 读锁），过长则拉长「就绪尾延迟」。建议 **默认值保持与现实现一致**，避免无配置升级时行为突变。

### 4.3 实现要点

- 在 `KubernetesCenter` / `FileSystemCenter` **构造或 start 入口** 解析一次，传入 spawn 的 `async move`（注意：`Copy` 的整数比 `Arc<Config>` 更易捕获）。
- 日志 **统一** 带上 `timeout_secs` 与（可选）`poll_interval_ms`，补齐 all-serve 缺失的 `timeout_secs`。

---

## 5. Revalidation 触发的统一方案

### 5.1 现状

以下三者在 **K8s 两处** 与 **FileSystem `start()`** 共 **三份** 相同调用序列：

- `trigger_full_cross_ns_revalidation`
- `trigger_gateway_secret_revalidation`
- `trigger_gateway_route_revalidation`

注释（cross-namespace / TLS secret / parentRefs）已在 leader-only 块中写全；all-serve 与 FileSystem **无同等注释**。

### 5.2 推荐

1. **单一函数**（例如 `resource_processor::run_post_cache_ready_revalidation()` 或 `conf_center::post_ready_revalidation::run()`），内部 **固定顺序** 调用上述三个 `trigger_*`，并在 **该函数文档** 中写明顺序理由（与现 leader-only 注释一致）。
2. K8s caches 任务、FileSystem `start()` **仅调用此函数**，消除三处序列漂移。
3. 若未来某些模式需 **跳过** 某一 trigger，用 **枚举或 feature 标志** 参数扩展，而不是再复制粘贴两行。

---

## 6. 推荐方案与实施步骤

### 6.1 推荐路径（分阶段，控制风险）

| 阶段 | 内容 | 风险 |
|------|------|------|
| **P0** | 提取 `run_post_cache_ready_revalidation().await`，三处替换；补全 all-serve 超时日志字段 | 低 |
| **P1** | 提取 `pending_sync_kinds` + `wait_processor_registry_ready(...)`（参数化 timeout + poll）；K8s 两处 spawn 共用；常量提到模块级 `const` 或配置 | 中 |
| **P2** | 配置项接入 + 文档默认值；FileSystem 与 K8s **对齐就绪语义**（传入 `expected_processor_kinds`） | 中–高（需集成/回归验证） |
| **P3** | 可选：`ProcessorRegistry` 抽象注入，对 `pending_sync_kinds` 做纯单元测试 | 低（工程投入） |

### 6.2 验收建议

- **行为**：leader-only 仍在就绪后启动 ACME；all-serve 仍不在 caches 任务内启动 ACME。
- **可观测性**：三种路径超时日志均含 `timeout_secs`、`not_ready`；必要时增加 `poll_interval_ms`。
- **回归**：大小集群启动、HA 切换、FileSystem reload 路径下，`ConfigSyncServer` 注册时机与 revalidation 与现网一致。

### 6.3 与既有评审文档的关系

- [03-event-watcher-duplication.md](./03-event-watcher-duplication.md) 已将 **caches 块** 列为与 leadership watcher、reload 并列的重复面；本评审将 **FileSystem** 纳入同一「就绪」语义，并单独强调 **判定差异、配置化与 revalidation 合一**。
- [04-sync-locks-in-async-context.md](./04-sync-locks-in-async-context.md) 已说明 `wait_registry_ready` 在 sleep 前后不长期持 center 锁；重构时 **保持不在持锁状态下 `await` 长轮询**。

---

## 7. 源码锚点（便于对照）

K8s leader-only 中超时常量、`pending_sync_kinds`、轮询、register、revalidation、ACME：

```517:583:Edgion/src/core/controller/conf_mgr/conf_center/kubernetes/center.rs
        let caches_handle = tokio::spawn(async move {
            const CACHE_READY_TIMEOUT_SECS: u64 = 30;
            let timeout = Duration::from_secs(CACHE_READY_TIMEOUT_SECS);
            // ... pending_sync_kinds, while sleep 200ms, register_all, trigger_* , start_acme_service, CachesReady / CachesTimeout
        });
```

K8s all-serve（无 ACME，超时日志缺 `timeout_secs`）：

```860:908:Edgion/src/core/controller/conf_mgr/conf_center/kubernetes/center.rs
        let caches_handle = tokio::spawn(async move {
            const CACHE_READY_TIMEOUT_SECS: u64 = 30;
            // ... 与上几乎相同的 pending + poll + register + trigger_* 
        });
```

FileSystem：`is_all_ready`、1s sleep、超时后 proceed；调用处 `wait_registry_ready(30)` 及后续三行 revalidation：

```128:168:Edgion/src/core/controller/conf_mgr/conf_center/file_system/center.rs
    async fn wait_registry_ready(&self, timeout_secs: u64) {
        // ... loop: is_all_ready -> return; timeout -> warn; sleep 1s
    }
```

```341:350:Edgion/src/core/controller/conf_mgr/conf_center/file_system/center.rs
            self.wait_registry_ready(30).await;
            // trigger_full_cross_ns_revalidation / secret / route
```

---

## 8. 小结

- **重复**：K8s 两条路径在 **就绪等待与 revalidation** 上为实质重复；FileSystem 在 **超时尺度与三条 trigger** 上与 K8s 同构，但 **判定条件、轮询间隔与代码结构** 不同。
- **硬编码**：30s 与 200ms/1s 分散在 const、参数与调用点，**未配置化**，不利于大集群与差异化部署。
- **方向**：先 **合一 revalidation** 与 **日志字段**，再 **提取等待例程 + 配置**，最后视需要 **统一 FileSystem 与 K8s 的就绪语义** 并补测试。

---

*文档版本：与 `kubernetes/center.rs`、`file_system/center.rs` 当前实现对照整理；行号随重构可能变化，请以符号搜索 `pending_sync_kinds`、`wait_registry_ready`、`CACHE_READY_TIMEOUT_SECS` 为准。*
