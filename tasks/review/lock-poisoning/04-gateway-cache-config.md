# Gateway Cache & Config — 锁中毒 Review

## 模块概述

本次 review 覆盖 Gateway 的**配置缓存层**和**配置同步层**，共 6 个文件：

| 模块 | 角色 | 路径温度 |
|---|---|---|
| `ClientCache<T>` | 泛型资源缓存，每种 K8s 资源一个实例 | **暖路径** — 仅 config sync（list/watch）及 admin API 读取 |
| `CacheData<T>` | ClientCache 内部数据 + debounce 事件压缩 | **暖路径** — tokio spawn 中的后台任务 |
| `event_dispatch.rs` | watch stream 事件应用 + list_and_reset | **暖路径** — gRPC sync 循环 |
| `ConfigClient` | 聚合所有 ClientCache + server_id / readiness | **暖路径** — sync 启动 + readiness probe |
| `EdgionGatewayConfigHandler` | 全局 `EDGION_GATEWAY_CONFIG_STORE` (LazyLock+RwLock) | **冷路径** — 仅 config sync 回调 + admin stats API |
| `GatewayClassHandler` | 全局 `GATEWAY_CLASS_STORE` (LazyLock+RwLock) | **冷路径** — 仅 config sync 回调 + admin stats API |

**关键事实**：这些锁**不在每请求热路径**上。请求路径的路由/插件/后端数据通过 `ConfHandler.full_set/partial_update` 写入各自的 ArcSwap store 后，由 ArcSwap 无锁读取。这些 `std::sync::RwLock` 仅在 config sync 循环、readiness 检查和 admin API 中被获取。

**但**：如果这些锁中毒，Gateway 将**永久丧失接收和应用配置更新的能力**，等于变成一台只能用启动时配置运行的僵尸代理。

## 锁使用清单

| # | 文件 | 行号 | 方法 | 锁类型 | 当前处理 | 风险等级 |
|---|---|---|---|---|---|---|
| 1 | `cache.rs` | 69 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 2 | `cache.rs` | 86 | `cache_data.write().unwrap()` | std RwLock | unwrap | 中 |
| 3 | `cache.rs` | 92 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 4 | `cache.rs` | 98 | `cache_data.write().unwrap()` | std RwLock | unwrap | 中 |
| 5 | `cache.rs` | 109 | `cache_data.write().unwrap()` | std RwLock | unwrap | 中 |
| 6 | `cache.rs` | 126 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 7 | `cache.rs` | 137 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 8 | `cache.rs` | 143 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 9 | `cache.rs` | 149 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 10 | `cache.rs` | 155 | `cache_data.read().unwrap()` | std RwLock | unwrap | 中 |
| 11 | `cache.rs` | 161 | `cache_data.write().unwrap()` | std RwLock | unwrap | 中 |
| 12 | `cache.rs` | 186 | `cache_data.write().unwrap()` | std RwLock (DynClientCache::set_ready) | unwrap | 中 |
| 13 | `cache_data.rs` | 131 | `cache_data.write().unwrap()` | std RwLock (process_compressed_events) | unwrap | **高** |
| 14 | `event_dispatch.rs` | 37 | `cache_data.write().unwrap()` | std RwLock (apply_change) | unwrap | **高** |
| 15 | `event_dispatch.rs` | 56 | `cache_data.write().unwrap()` | std RwLock (set_ready) | unwrap | 中 |
| 16 | `event_dispatch.rs` | 117 | `cache_data.write().unwrap()` | std RwLock (list_and_reset) | unwrap | **高** |
| 17 | `event_dispatch.rs` | 187 | `cache_data.read().unwrap()` | std RwLock (start_watch 内 list 成功后) | unwrap | 中 |
| 18 | `event_dispatch.rs` | 205 | `cache_data.write().unwrap()` | std RwLock (set_ready in watch) | unwrap | 中 |
| 19 | `event_dispatch.rs` | 333 | `cache_data.write().unwrap()` | std RwLock (watch event apply) | unwrap | **高** |
| 20 | `config_client.rs` | 182 | `required_cache_names.write().unwrap()` | std RwLock | unwrap | 低 |
| 21 | `config_client.rs` | 217 | `current_server_id.read().unwrap()` | std RwLock | unwrap | 低 |
| 22 | `config_client.rs` | 222 | `current_server_id.write().unwrap()` | std RwLock | unwrap | 低 |
| 23 | `config_client.rs` | 375 | `required_cache_names.read().unwrap()` | std RwLock | unwrap | 低 |
| 24 | `edgion_gateway/conf_handler_impl.rs` | 14 | `EDGION_GATEWAY_CONFIG_STORE.read().unwrap()` | LazyLock+RwLock (count) | unwrap | 中 |
| 25 | `edgion_gateway/conf_handler_impl.rs` | 20 | `EDGION_GATEWAY_CONFIG_STORE.read().unwrap()` | LazyLock+RwLock (get_by_name) | unwrap | 中 |
| 26 | `edgion_gateway/conf_handler_impl.rs` | 26 | `EDGION_GATEWAY_CONFIG_STORE.read().unwrap()` | LazyLock+RwLock (list) | unwrap | 中 |
| 27 | `edgion_gateway/conf_handler_impl.rs` | 53 | `EDGION_GATEWAY_CONFIG_STORE.write().unwrap()` | LazyLock+RwLock (full_set) | unwrap | **高** |
| 28 | `edgion_gateway/conf_handler_impl.rs` | 63 | `EDGION_GATEWAY_CONFIG_STORE.write().unwrap()` | LazyLock+RwLock (partial_update) | unwrap | **高** |
| 29 | `gateway_class/conf_handler_impl.rs` | 14 | `GATEWAY_CLASS_STORE.read().unwrap()` | LazyLock+RwLock (count) | unwrap | 中 |
| 30 | `gateway_class/conf_handler_impl.rs` | 20 | `GATEWAY_CLASS_STORE.read().unwrap()` | LazyLock+RwLock (get_by_name) | unwrap | 中 |
| 31 | `gateway_class/conf_handler_impl.rs` | 26 | `GATEWAY_CLASS_STORE.read().unwrap()` | LazyLock+RwLock (list) | unwrap | 中 |
| 32 | `gateway_class/conf_handler_impl.rs` | 53 | `GATEWAY_CLASS_STORE.write().unwrap()` | LazyLock+RwLock (full_set) | unwrap | **高** |
| 33 | `gateway_class/conf_handler_impl.rs` | 63 | `GATEWAY_CLASS_STORE.write().unwrap()` | LazyLock+RwLock (partial_update) | unwrap | **高** |

**合计**：33 个 `.unwrap()` 站点，0 个有中毒恢复处理。

## 问题分析

### 1. 中毒概率评估：实际极低

`std::sync::RwLock` 仅在持锁期间线程 panic 时才会中毒。审查所有临界区后发现：

- **cache.rs / cache_data.rs**：临界区内的操作是 `HashMap::insert/remove/get`、`iter().map().collect()`、布尔赋值。这些操作不会 panic，除非 OOM（极端情况）。
- **event_dispatch.rs**：临界区稍重（在 apply_change 中有 `key_name()` 和 `add_compress_event`；在 watch event processing 中有 `serde_json::from_value` 但这在锁外执行）。实际 panic 概率极低。
- **conf_handler_impl.rs**（两个全局 store）：临界区内只有 `HashMap` 操作和 `clone()`，不会 panic。
- **config_client.rs**：临界区内只有 `String` clone、`HashSet` 操作，不会 panic。

**核心矛盾**：中毒概率极低，但一旦发生后果极严重（全局静态 store 无法替换，config sync 永久中断）。

### 2. LazyLock 单例加剧后果

`EDGION_GATEWAY_CONFIG_STORE` 和 `GATEWAY_CLASS_STORE` 是 `LazyLock<RwLock<...>>` 全局静态变量。一旦其中的 RwLock 中毒：
- 无法重新初始化（LazyLock 只初始化一次）
- 所有后续 `full_set` / `partial_update` 调用都会 panic
- 级联效应：`process_compressed_events`（tokio spawn 后台任务）panic -> 持有 `cache_data` 的写锁 -> `cache_data` 也中毒 -> 整个 ClientCache 瘫痪

### 3. 传播链分析

如果 `ConfHandler.full_set()` 或 `partial_update()` 内部 panic（例如全局 store 的 `.write().unwrap()` 因已中毒而 panic）：

```
full_set/partial_update panic
  -> 在 CacheData::reset() 或 process_compressed_events() 中被调用
     -> 这些函数持有 cache_data 的写锁
        -> cache_data RwLock 也中毒
           -> ClientCache 的所有方法（is_ready, get, list, reset 等）全部 panic
              -> Config sync 循环崩溃
                 -> Gateway 永久无法接收配置更新
```

### 4. 与热路径的关系

好消息：这些锁**不在请求热路径**上。实际数据面读取走的是各 store 的 ArcSwap（如 RouteManager、PluginStore 等），由 `ConfHandler` 写入后请求侧无锁读取。

但 `count_gateway_classes()` 和 `count_edgion_gateway_configs()` 被 admin stats API 调用（`/api` 端点），如果 store 中毒，stats API 也会 panic。

### 5. `base_conf` 字段

`ConfigClient.base_conf: RwLock<Option<GatewayBaseConf>>` 在代码中从未被使用（零引用），是死代码。

## 最佳处理方案

### 方案：统一切换到 `parking_lot::RwLock`

**理由**：

1. 项目已依赖 `parking_lot = "0.12"`（`Cargo.toml:130`），但这些模块仍用 `std::sync::RwLock`。
2. `parking_lot::RwLock` **不会中毒**——如果持锁线程 panic，锁自动释放，后续获取正常进行。
3. API 完全兼容，`.read()` 和 `.write()` 直接返回 guard（无 `Result`），所以切换后所有 `.unwrap()` 自然消失。
4. 性能更优：`parking_lot` 在无竞争和轻竞争场景下比 `std` 更快。

### 具体改动

**文件 1: `cache.rs`**
```rust
// 之前
use std::sync::{Arc, RwLock};
// 之后
use std::sync::Arc;
use parking_lot::RwLock;
```
所有 `.read().unwrap()` -> `.read()`，`.write().unwrap()` -> `.write()`。

**文件 2: `cache_data.rs`**
```rust
// 之前
use std::sync::{Arc, RwLock};
// 之后
use std::sync::Arc;
use parking_lot::RwLock;
```
`process_compressed_events` 中的 `.write().unwrap()` -> `.write()`。

**文件 3: `event_dispatch.rs`**
```rust
// 之前
use std::sync::{Arc, RwLock};
// 之后
use std::sync::Arc;
use parking_lot::RwLock;
```
6 处 `.unwrap()` 全部移除。

**文件 4: `config_client.rs`**
```rust
// 之前
use std::sync::RwLock;
// 之后
use parking_lot::RwLock;
```
4 处 `.unwrap()` 全部移除。同时考虑删除未使用的 `base_conf` 字段。

**文件 5-6: 两个 `conf_handler_impl.rs`**
```rust
// 之前
use std::sync::RwLock;
static STORE: LazyLock<RwLock<...>> = ...;
// 之后
use parking_lot::RwLock;
static STORE: LazyLock<RwLock<...>> = ...;
```
各 5 处 `.unwrap()` 移除。`LazyLock<parking_lot::RwLock<...>>` 完全合法。

### 注意事项

- `tokio::sync::RwLock`（`grpc_client` 字段）**不需要改**，它用 `.await` 而非 `.unwrap()`，且本身不会中毒。
- 测试文件中 `TEST_MUTEX: std::sync::Mutex<()>` 可以保留——测试中 `Mutex` 中毒是可接受的（用来序列化全局 store 测试）。
- `cache_data.rs` 中 `set_conf_processor` 的签名参数 `cache_data: Arc<RwLock<CacheData<T>>>` 需要同步更新为 `parking_lot::RwLock`。

## 优先级

| 优先级 | 项目 | 理由 |
|---|---|---|
| **P2** | 全部 33 处切换到 `parking_lot::RwLock` | 中毒概率低但后果致命（config sync 永久中断），改动纯机械替换，风险极低 |
| **P3** | 删除 `ConfigClient.base_conf` 死代码 | 减少维护负担，但不影响正确性 |

**补充说明**：虽然标记为 P2 而非 P1，是因为临界区内的操作在正常运行条件下不会 panic。但这是一个"防御性编程"问题——使用不会中毒的锁实现从根本上消除了这一类风险，而代价几乎为零（已有依赖、API 兼容、性能更优）。建议在下一个维护窗口统一处理。
