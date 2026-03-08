# Controller 侧内存泄漏 Review

本文件只记录与 `Controller / ConfMgr / ConfigSync` 相关、且和“任务堆积 / 全局状态残留 / reload 后无法回收”直接相关的问题。

## 1. 严重，已确认：Workqueue 前面包了一层 detached `tokio::spawn`，高压时会在队列外堆积任务

- 文件：
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs`
  - `src/core/controller/conf_mgr/sync_runtime/workqueue.rs`
- 相关符号：
  - `ResourceProcessor::on_apply()`
  - `ResourceProcessor::on_delete()`
  - `ProcessorObj::requeue()`
  - `ProcessorObj::requeue_with_chain()`
  - `ProcessorObj::requeue_all_keys()`
  - `Workqueue::enqueue()`
  - `Workqueue::enqueue_after()`

### 现象

外部调用不是“直接调用 `workqueue.enqueue(...).await`”，而是先 `tokio::spawn(async move { workqueue.enqueue(...).await })`。而 `enqueue()` 自身内部又会等待有界 channel 的 `send().await`。

### 为什么这是泄漏

有界的是 channel，不是外层任务数。

也就是说，当队列满或 worker 处理慢时：

- channel 中的元素是有限的
- 但外层等待 `send().await` 的 detached task 可以无限长队

这些 task：

- 没有统一句柄
- 没有批量取消
- 不计入 workqueue depth 指标

因此内存和调度负载会在“队列外”继续增长。

### 触发条件

- K8s watcher 突发事件
- `requeue_all()` 触发全量回灌
- Secret / Service / ReferenceGrant 引发大量级联 requeue
- 处理链路短暂变慢

### 风险结论

这是本轮 Controller 侧最明确、最危险的增长点，属于“bounded queue + unbounded waiting tasks”的典型设计缺陷。

### 修复建议

- 去掉外围 `tokio::spawn`
- 将 requeue 请求统一交给单一调度器处理
- 或者显式限制同时存在的 enqueue task 数量

## 2. 严重，已确认：`ReferenceGrantStore` 与 `CrossNamespaceRefManager` 是进程级单例，但 `clear_registry()` 不清理

- 文件：
  - `src/core/controller/conf_mgr/processor_registry.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_grant/store.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/ref_grant/cross_ns_ref_manager.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/reference_grant.rs`

### 现象

`clear_registry()` 目前只清：

- processors
- `ListenerPortManager`
- `ServiceRefManager`

但没有清：

- `ReferenceGrantStore`
- `CrossNamespaceRefManager`

而这两个结构都是 `OnceLock` / 全局单例，生命周期就是整个进程。

### 为什么这是泄漏

当 controller 发生：

- relink
- 失去 leader 后重新建立服务
- reload / watcher 重建

旧 epoch 中已不存在的 `ReferenceGrant`、跨命名空间引用关系，如果没有收到 delete 事件，就会残留在全局单例中。

这会导致两类问题：

- 内存残留
- 权限 / 依赖关系判断被历史数据污染

### 证据链

- `clear_registry()` 未触达这两个全局对象
- `ReferenceGrantHandler::parse()` 只做 `store.upsert(...)`
- `CrossNamespaceRefManager` 的清理依赖资源级 `clear_resource_refs()`，但对“上一轮存在、这一轮已经消失”的对象无能为力

### 风险结论

这是典型的“reload 后全局单例残留”，属于确认问题。

### 修复建议

- 给这两个全局单例补 `clear()` / `replace_all()` 入口
- 在 `clear_registry()` 或新一轮 init 前做权威式重置

## 3. 高，已确认：`GatewayRouteIndex` 和 `AttachedRouteTracker` 是全局单例，但 reload 后没有完整重建路径

- 文件：
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/gateway_route_index.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/attached_route_tracker.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/processor.rs`
  - `src/core/controller/conf_mgr/processor_registry.rs`

### 现象

这两个索引都是 `LazyLock` 全局单例。它们的 `clear()` 只存在于 `#[cfg(test)]`。与此同时，`process_resource()` 在 `is_init == true` 时不会调用 `handler.on_change()`。

### 为什么这是问题

这些索引主要依赖 route/gateway 的 `on_change()` 去增量维护；但 controller 重建后的 init 阶段：

- 不会调用 `on_change()`
- 也没有全局 clear

因此上一轮残留索引项可能继续存在，而当前轮新对象又未必能完整重建索引。

### 风险结论

这不只是状态不一致，更是全局 map 的长期残留，运行时间越久、重建次数越多，越容易把历史数据沉淀在内存里。

### 修复建议

- 给这两个索引增加生产可用的 `clear()`
- 在 controller 新 epoch 初始化前先清空
- 或改成 init 阶段 `replace_all` / authoritative rebuild

## 4. 高风险疑点：`SecretStore` 是全局常驻缓存，但初始化阶段没看到稳定的全量替换

- 文件：
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/secret_utils/secret_store.rs`
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/secret.rs`

### 现象

`SecretStore` 是全局 `LazyLock<Arc<SecretStore>>`。`SecretHandler::parse()` 在 init 阶段只做：

- 单个 secret 的 `update_secrets(upsert, empty_remove)`

没有看到在 controller 重建时稳定执行的权威式 `replace_all()`。

### 为什么值得担心

如果某些 Secret 在 controller 离线、relink、watch 重建窗口中已经被删除，那么：

- 新一轮 init 只会 upsert 还存在的对象
- 旧 secret 若没收到 delete，可能永久留在全局 store

### 当前判断

我把它记为“高风险疑点”，因为 `SecretStore` 自身提供了 `replace_all()`，但本轮还没沿启动路径确认它是否在别处被稳定调用。

### 修复建议

- 明确 init 阶段改为 `replace_all()`
- 或在 registry 重置时显式清空 secret 全局缓存

## 5. 高风险疑点：`NamespaceStore` 也是全局常驻缓存，但 namespace watcher 是流式增量更新

- 文件：
  - `src/core/controller/conf_mgr/sync_runtime/resource_processor/namespace_store.rs`
  - `src/core/controller/conf_mgr/conf_center/kubernetes/controller.rs`

### 现象

`NamespaceStore` 是进程级全局 store。namespace watcher 用的是 `applied_objects()` 流式消费，处理逻辑是：

- 有删除时间戳则 `remove`
- 否则 `upsert`

但本轮没有看到 watcher 启动时先做一次全量 `replace_all()`。

### 为什么值得担心

如果某个 namespace 在 watcher 重启窗口里已经消失，而新流里又不会重新播发它，那么旧条目就可能一直残留。

### 当前判断

这条也是“高风险疑点”，原因和 `SecretStore` 类似：设计上已有 `replace_all()`，但实际初始化链路未看到稳定使用。

### 修复建议

- namespace watcher 启动前先 list + `replace_all()`
- 或在 leader 切换时清空 namespace 全局缓存

## 已排除项

### ConfigSync 主链路 channel

- 结论：目前看到的 channel 大多是有界的，而且主链路有断连退出路径
- 说明：本轮没有把它们列为主问题

### `ServerCache.watchers`

- 结论：存在 lazy prune，但更像“清理不积极”，还不构成当前最危险问题

### `requeue_with_backoff()`

- 结论：它也会再 `spawn` 一个 sleep task，但相较于通用 `on_apply/requeue/requeue_all_keys` 链路，当前生产风险小得多

## 建议优先级

1. 优先修 `detached enqueue task` 堆积问题
2. 紧接着补齐 `clear_registry()` 对全局单例的清理
3. 把 route / gateway 索引从“增量维护”改成“可重建”
4. 第二轮继续把 `SecretStore` / `NamespaceStore` 的启动链路追到底，确认是否真的缺少全量替换
