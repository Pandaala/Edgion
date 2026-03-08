# 本轮已排除的误报点

本文件记录本轮 review 过程中看起来“像泄漏”，但在当前证据下我没有升级为问题的点，避免后续重复排查。

## 1. `access_log_store` 不是无界增长

- 文件：`src/core/gateway/observe/access_log_store.rs`
- 原因：已有 TTL、容量上限和清理逻辑
- 结论：本轮不判为泄漏

## 2. 当前本地日志 sink 不是无限堆内存

- 关注点：异步日志队列是否会因下游写盘慢而无限涨
- 当前结论：本地文件写入链路采用有界队列，满了更偏向丢弃，不是无限占用堆内存

## 3. webhook manager 的活跃检查任务具备替换 / 回收能力

- 文件：`src/core/gateway/link_sys/providers/webhook/manager.rs`
- 原因：`upsert/remove` 时会停止旧任务
- 结论：暂不列为泄漏

## 4. backend health check manager 暂未见典型残留

- 文件：`src/core/gateway/backends/health/check/manager.rs`
- 原因：旧任务有 `abort()`，状态存储有相应 unregister 逻辑
- 结论：本轮不升级

## 5. OIDC 内部并非所有缓存都无界

- 文件：`src/core/gateway/plugins/http/openid_connect/openid_impl.rs`
- 说明：本轮真正标红的是 `introspection_cache`
- 已排除：
  - `access_token_cache` 有 `>4096` 时清空保护
  - singleflight 相关 map 也看到了清理逻辑

## 6. ConfigSync 的主 watch channel 目前不算最危险增长点

- 原因：大多是有界 channel，且主链路有断连退出逻辑
- 说明：仍值得第二轮复查，但暂不列为本轮主问题

## 7. `ServerCache.watchers` 更像“清理不积极”，不是本轮最强证据

- 原因：stale watcher 在后续调用中有机会被惰性清理
- 结论：保留观察，不作为本轮主结论

## 8. `requeue_with_backoff()` 的 sleep task 不是本轮主问题

- 文件：`src/core/controller/conf_mgr/sync_runtime/workqueue.rs`
- 原因：虽然也会派生任务，但生产路径上不如通用 requeue 链路高频、明确、危险
- 结论：先不单独升级

## 后续复查建议

如果继续做第 2 轮，可以把下面几类作为“待证伪”目标继续追：

- LinkSys provider 的连接池和后台任务释放
- ConfigSync client/server 在异常断链、频繁重连下的残留句柄
- Backend discovery 对历史 endpoint / slice 的 authoritative prune
- 插件内部所有 `DashMap` / `RwLock<HashMap<...>>` 的容量边界
