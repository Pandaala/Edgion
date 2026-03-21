# Step 01 - Gateway Overview And Current Caches

## Facts

### 1. Gateway 主链路本身已经大量依赖 cache/store

Edgion 的 gateway 不是“无状态直通代理”，而是明显依赖多类本地快照和运行态 cache：

- 配置同步侧有按资源种类拆分的 `ClientCache<T>`，负责 `list/watch` 后的本地资源快照、sync version、ready 状态、事件压缩与 `ConfHandler` 分发。
  - 代码参考：`src/core/gateway/conf_sync/cache_client/cache.rs:31-172`
- Gateway 资源会进入动态查找 store，用于 listener/host/port/TLS 的快速匹配与热更新。
  - 代码参考：`src/core/gateway/runtime/handler.rs`
- HTTP / gRPC / TCP / UDP / TLS 路由管理器都维护各自的路由快照与匹配结构，避免每次请求重新解析资源。
  - HTTP 路由表与 host/radix/regex 结构：`src/core/gateway/routes/http/routes_mgr.rs`
  - gRPC `route_units_cache`：`src/core/gateway/routes/grpc/routes_mgr.rs:151-199`
- 插件资源有全局 `PluginStore` 与 `StreamPluginStore`，用于原子替换、热更新与 `ExtensionRef` / stream plugin 的运行时查找。
  - `src/core/gateway/plugins/http/plugin_store.rs:19-82`
  - `src/core/gateway/plugins/http/conf_handler_impl.rs:31-65`
  - `src/core/gateway/plugins/stream/stream_plugin_store.rs`
- LB 层有显式 runtime state cache，缓存 service 级 EWMA、连接数、backend 生命周期状态、RR selector、consistent hash ring。
  - `src/core/gateway/lb/runtime_state/mod.rs:1-139`
- 观测与辅助服务也有小型 cache/store：
  - ACME challenge store：`src/core/gateway/services/acme/challenge_store.rs`
  - access log TTL store（测试/调试）：`src/core/gateway/observe/access_log_store.rs`
  - TLS store / backend TLS store / endpoint store / service store / link_sys store

### 2. 请求处理路径建立在“预解析 + 快照查找”之上

请求路径中，真正热路径依赖的是预构建 cache，而不是临时计算：

- `request_filter` 先用全局 route table 做路由匹配，再执行 gateway global plugins 与 route plugins。
  - `src/core/gateway/routes/http/proxy_http/pg_request_filter.rs:26-129`
- `PluginStore` 在 `full_set/partial_update` 时会先 `preparse()`，把 plugin runtime 和配置期缓存准备好，再原子替换进 store。
  - `src/core/gateway/plugins/http/conf_handler_impl.rs:31-65`

这意味着 Edgion 当前已经具备一个重要前提：

- 很适合继续加“配置期 cache / 编译期 cache / 热路径查表 cache”
- 但还没有通用“上游内容 cache（HTTP response object cache）”

### 3. 目前不存在通用 HTTP 内容缓存能力

从当前 gateway 结构看：

- 已有 route cache、selector cache、credential cache、regex/bytecode cache、rate state cache
- 没有看到统一的 `proxy_cache`/`response object cache`/`cache key + storage + revalidate + stale` 体系
- `EdgionPlugin` 里也明确留了 `EdgionCache(CacheConfig)` 的 TODO，占位但未实现
  - `src/types/resources/edgion_plugins/edgion_plugin.rs`

这说明现在的“cache”主要还是：

- 配置快照 cache
- 运行态查找 cache
- 插件内部局部 cache

而不是传统 CDN / reverse proxy 的对象缓存。

## Cache Types Already Present In Gateway

| 类型 | 作用 | 当前例子 |
|---|---|---|
| 资源快照 cache | 把 controller 下发资源保存在 gateway 本地 | `ClientCache<T>` |
| 原子替换 store | 热更新时无锁读、低成本切换 | `PluginStore`, `StreamPluginStore`, route table ArcSwap |
| 预解析/编译 cache | 把 regex / runtime / bytecode 在配置阶段准备好 | plugin `preparse()`, route unit cache |
| 路由匹配 cache | host/path/regex 匹配结构复用 | HTTP route manager, gRPC route units cache |
| LB runtime state cache | 维持请求间统计与 selector 对象 | EWMA, RR, CH ring, conn count |
| TTL 临时 store | 调试与测试辅助 | access log store, ACME challenge store |
| LinkSys client store | 复用外部系统 client | Redis/Etcd/Webhook/ES store |

## What Is Missing

当前缺失或仅部分具备的 cache 能力：

- 通用 HTTP response content cache
- request body cache / spool
- plugin 级统一 TTL cache 抽象
- 跨节点共享 cache 抽象
- cache stampede 防护的统一封装
- stale-while-revalidate / stale-if-error 语义
- 统一的 cache metrics / hit ratio / eviction / memory budget 管理

## Initial Judgment

如果后续要给 Edgion 增加 cache 能力，应该至少拆成四层，而不是只做一个“大而全”的 `EdgionCache`：

1. 配置期编译 cache
2. 节点内运行态 cache
3. 分布式共享状态 cache
4. 上游响应内容 cache

这四层的 key、TTL、失效机制、容量控制、正确性边界都不同。

## Current Issues

- 目前各插件各自实现局部 cache，能力分散，策略不统一
- 大部分 cache 都是单节点内存态，天然不具备跨 gateway 一致性
- 热更新后如何精确失效，依赖各插件各自处理，没有统一框架
- 还没有 request body cache，因此 body 参与签名/验签/去重类插件会受限

## Risks

- 如果未来把“认证结果 cache”“限流状态 cache”“响应内容 cache”混在一个抽象里，极容易做错失效模型
- 如果直接把本地内存 cache 用于需要全局一致性的限流/配额场景，会产生多节点放大
- 如果未来引入响应缓存但没有 `lock/stale/revalidate` 机制，容易出现 cache stampede

