# Gateway 侧内存泄漏 Review

本文件只记录与 `Gateway` 常驻运行时相关、且和“内存无法回收 / 无界增长 / 生命周期失控”直接相关的问题。

## 1. 严重，已确认：UDP 清理协程会把整个 `EdgionUdp` 实例永久保活

- 文件：`src/core/gateway/routes/udp/edgion_udp.rs`
- 相关符号：`EdgionUdp::serve()`、`EdgionUdp::session_cleanup_loop()`

### 现象

`serve()` 启动时会额外 `spawn` 一个 session 清理协程，并把 `self.clone()` 传入该协程。这个清理协程是无限循环，没有 shutdown、socket closed、listener removed 等退出条件。

### 为什么这是泄漏

只要 `serve()` 所在的主接收循环退出，那个后台清理任务仍然会继续持有 `Arc<Self>`。这意味着：

- `socket`
- `client_sessions`
- `gateway_udp_routes`
- `edgion_gateway_config`

这些本应随 listener 生命周期结束而释放的对象，会被后台协程继续持有，导致整条 UDP 服务对象链条无法回收。

### 触发条件

- UDP listener 因 reload 被替换
- socket 出错导致 `recv_from` 循环退出
- 运行时关闭某个 listener

### 证据

- `serve()` 中：
  - 克隆 `self`
  - `tokio::spawn(async move { cleanup_self.session_cleanup_loop().await; })`
- `session_cleanup_loop()` 中：
  - `loop { sleep(10s).await; ... }`
  - 没有任何 break 条件

### 风险结论

这是标准的“后台任务把 owner 反向保活”的生命周期泄漏，严重级别应最高优先修复。

### 修复建议

- 为 `EdgionUdp` 增加 shutdown signal 或 cancellation token
- 在 `serve()` 退出前显式通知 cleanup task 停止
- 或者不要让 cleanup task 持有整个 `Arc<Self>`，只持有所需的最小字段

## 2. 高，已确认：`BasicAuth` 成功认证缓存只有逻辑 TTL，没有物理淘汰

- 文件：`src/core/gateway/plugins/http/basic_auth/plugin.rs`
- 相关符号：`BasicAuth::auth_cache`、`authenticate_request()`

### 现象

缓存 key 是完整 `Authorization` header，value 是 `(username, expiry)`。访问时如果命中且没过期则返回；如果命中且已过期，才会删除当前 key。

### 为什么这是泄漏

TTL 只决定“这个 key 还能不能用”，并不决定“这个 key 会不会自动从 map 里清掉”。对于“一次命中后再也不访问”的 header：

- 逻辑上会过期
- 物理上不会自动删除

因此缓存大小会和历史唯一成功认证 header 数量一起增长，直到插件实例重建。

### 典型场景

- 多租户、多用户环境
- 很多一次性或低复用凭据
- 认证头包含不同用户名密码组合

### 风险结论

这是“冷 key 永不淘汰”的典型长期累积问题。

### 修复建议

- 增加容量上限，比如 LRU / TinyLFU / bounded DashMap 封装
- 定期 `retain()` 清理过期项
- 把缓存 key 降维，不直接使用完整 header 原文

## 3. 高，已确认：`LdapAuth` 成功认证缓存同样存在冷 key 永久滞留

- 文件：`src/core/gateway/plugins/http/ldap_auth/plugin.rs`
- 相关符号：`LdapAuth::auth_cache`、`check_cache()`、`store_cache()`

### 现象

缓存 key 是 `SHA-256(username:password)`，逻辑上比 `BasicAuth` 更安全，但淘汰策略和 `BasicAuth` 一样：只有再次访问相同 key 时，才会发现它过期并删除。

### 为什么这是泄漏

只要历史上出现过很多唯一用户名密码组合，即使这些条目早就失效，它们也会一直留在 `DashMap` 中。

### 风险结论

这不是瞬时峰值问题，而是进程运行时间越长、成功认证组合越多，内存越难回收。

### 修复建议

- 与 `BasicAuth` 一样，加容量上限
- 增加周期性清理协程，或在写入时基于阈值执行 `retain()`
- 将缓存命中策略与淘汰策略解耦

## 4. 高风险疑点：EWMA 的 per-backend 全局状态缺少稳定回收路径

- 文件：`src/core/gateway/lb/ewma/metrics.rs`
- 相关符号：`EWMA_VALUES`、`update()`、`remove()`

### 现象

`EWMA_VALUES` 是进程级 `LazyLock<DashMap<SocketAddr, AtomicU64>>`。每个 backend 地址第一次被观察到时都会插入。虽然实现了 `remove()`，但在生产路径中没有看到稳定的调用链把已消失 backend 的地址移除。

### 为什么值得担心

在 Kubernetes 场景里，backend 地址天然会 churn：

- Pod 重建
- 滚动发布
- EndpointSlice 频繁变化

如果历史地址只增不删，这个 map 会跟着历史后端总数持续增长。

### 当前判断

- `EWMA_VALUES` 的增长是明确的
- 生产路径中的可靠回收证据不足
- 因此先定为“高风险疑点”

### 修复建议

- 在 backend 集合变更时做 authoritative prune
- 或在 selector / discovery 层拿到最新 backend 集合后做差集删除

## 5. 高风险疑点：LeastConn 的计数表依赖 draining 流程清理，但生产路径没有看到稳定触发

- 文件：`src/core/gateway/lb/leastconn/counter.rs`
- 配套文件：`src/core/gateway/lb/leastconn/cleaner.rs`
- 相关符号：`CONNECTION_COUNTS`、`increment()`、`decrement()`、`BackendCleaner::start()`

### 现象

连接计数为 0 时不会自动删除 entry，真正删除依赖 cleaner。cleaner 只处理“已经进入 draining 状态”的 backend。

### 为什么值得担心

如果 backend 从活跃集合消失，但没有先被标记为 draining，那么：

- 计数可能已经回到 0
- 但 map entry 仍会长期存在

我检索到 `mark_draining()` 在测试里有调用，但生产代码中没有看到明确、稳定的上游触发链。

### 当前判断

这条比 EWMA 更偏“高风险疑点”，因为需要结合 backend 生命周期管理代码再做第二轮核实。

### 修复建议

- backend 删除时直接清理 counter / state
- 不要只依赖 draining 分支做收尾

## 6. 中高，高风险疑点：OIDC introspection cache 没有容量上限

- 文件：`src/core/gateway/plugins/http/openid_connect/openid_impl.rs`
- 相关符号：`introspection_cache`、`get_cached_introspection_claims()`、`cache_introspection_claims()`

### 现象

写入时会 `retain()` 掉已过期项，但没有 `max_entries`、LRU、近似容量控制。过期项也只会在后续写入或访问同 key 时被清掉。

### 为什么值得担心

如果开启 introspection cache 且 bearer token 的唯一值很多，那么缓存上界接近于：

`TTL 窗口内出现的唯一 token 数`

一旦 TTL 配得偏大，或者流量基数偏高，内存会快速膨胀。

### 当前判断

严格来说这是“无上限缓存设计缺陷”，是否表现成泄漏取决于流量模型，但风险很高。

### 修复建议

- 增加 `max_entries`
- 触顶时按 LRU / FIFO 淘汰
- TTL 配置增加上限校验

## 7. 中高，高风险疑点：UDP 路径对 session / socket / task 缺少硬上限

- 文件：`src/core/gateway/routes/udp/edgion_udp.rs`
- 相关符号：`client_sessions`、`get_or_create_session()`、`handle_upstream_packets_static()`

### 现象

每个新的客户端地址都会创建：

- 1 个 `ClientSession`
- 1 个 `UdpSocket`
- 1 个上游监听 task

此外，每个入包还会额外 `tokio::spawn` 一个处理任务。

### 为什么值得担心

虽然有 60 秒 idle timeout，但它只对“静默会话”有效。如果遇到高基数、持续活跃、甚至伪造源地址的 UDP 流量：

- session 数量会跟活跃地址数线性增长
- socket 数量和 task 数量同步增长
- 没有任何 max sessions / fd cap / backpressure

### 当前判断

这更像“资源上限缺失导致的内存和 FD 爆炸”，不完全等同传统泄漏，但在生产表现上会非常像泄漏。

### 修复建议

- 增加全局最大 session 数
- 给 listener 增加并发限制
- 超限时优先回收最旧或最不活跃 session

## 已排除项

### `access_log_store`

- 文件：`src/core/gateway/observe/access_log_store.rs`
- 结论：已看到 TTL、容量上限和清理逻辑，不属于本轮重点问题

### 本地日志异步 sink

- 结论：当前本地文件写入链路是有界队列，队列满时偏向丢弃，不是无限堆内存

### webhook 健康检查任务

- 文件：`src/core/gateway/link_sys/providers/webhook/manager.rs`
- 结论：`upsert/remove` 时会中止旧 task，未见明显残留

### health check manager

- 文件：`src/core/gateway/backends/health/check/manager.rs`
- 结论：旧任务有 `abort()`，状态存储也有 unregister 行为，本轮不判为泄漏

## 建议优先级

1. 先修 `UDP cleanup task` 的生命周期泄漏
2. 再修 `Controller/Workqueue` 的 detached task 堆积问题，因为它也会放大 Gateway reload 频率
3. 给 `BasicAuth` / `LdapAuth` / `OIDC introspection cache` 加上界
4. 第二轮继续核实 LB 历史 backend 状态的 authoritative prune 机制
