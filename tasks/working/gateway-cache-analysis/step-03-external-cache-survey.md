# Step 03 - External Cache Survey

## 1. Cache 类型先分层

在网关体系里，常见 cache 至少分为四大类：

### A. Proxy / Content Cache

- 缓存的是上游响应对象
- 关键点是 `cache key`、`TTL`、`Vary`、`revalidate`、`stale`、`purge`
- 典型代表：Nginx `proxy_cache`、Pingora `pingora_cache`

### B. Local In-Memory Runtime Cache

- 缓存的是本地进程里的对象、查表结果、编译产物
- 关键点是并发、内存上限、驱逐策略、TTL/TTI
- 典型代表：Moka、mini-moka、quick_cache、lru、TinyUFO

### C. Distributed Shared State Cache

- 缓存的是多节点共享状态
- 关键点是一致性、原子操作、过期、锁、单飞、防止超发
- 典型代表：Redis、Etcd

### D. Request/Response Buffer Cache

- 不是传统 KV cache，而是为请求体/响应体的二次读取、重放、验签、异步处理提供缓冲/落盘
- 在 API gateway / WAF / auth gateway 中经常是必须能力

## 2. Pingora 相关 cache

### 2.1 `pingora_cache`

根据 docs.rs，`pingora_cache` 是 “The HTTP caching layer for proxies”。

关键点：

- 面向代理的 HTTP cache，不是普通业务 KV cache
- 暴露了 `CacheKey`、`CacheMeta`、`Storage`、`HitHandler`、`MissHandler`
- 模块里明确包含：
  - `lock`
  - `predictor`
  - `storage`
  - `filters`
  - `eviction`
  - `MemCache`
  - `HttpCache` state machine

含义：

- 它提供的是“完整代理缓存语义”
- 包含 cacheability 判断、命中/未命中路径、锁、防击穿、存储抽象
- 适合做上游响应缓存、反向代理 cache、CDN/edge cache 能力

参考：

- [pingora_cache docs.rs](https://docs.rs/pingora-cache/latest/pingora_cache/)
- 关键描述见 `docs.rs` 页面中的 crate description 与 modules

### 2.2 `pingora-memory-cache`

Pingora 仓库 README 明确写到：

- `pingora-memory-cache`: “Async in-memory caching with cache lock to prevent cache stampede”

docs.rs 页面进一步说明：

- `MemoryCache`: “A high performant in-memory cache with S3-FIFO + TinyLFU”
- `RTCache`: read-through in-memory cache

含义：

- 更偏节点内 hot object cache
- 适合解决热点对象的本地加速与 miss 协调
- 非常适合做多级 cache 里的 L1
- 自带 cache lock 语义，适合防止热点 key 同时回源

参考：

- [Pingora 仓库 README](https://github.com/cloudflare/pingora)
- [pingora-memory-cache docs.rs](https://docs.rs/pingora-memory-cache/latest/pingora_memory_cache/)

### 2.3 TinyUFO

TinyUFO 是 Pingora 生态里的底层内存 cache 算法实现。

README 说明：

- 是高性能内存 cache
- 采用 `S3-FIFO` 与 `TinyLFU`
- 目标是高吞吐和高命中率

作用：

- 适合做网关里的热点对象、本地对象 cache
- 更偏 cache engine，而不是完整 proxy cache 框架

参考：

- [TinyUFO README](https://raw.githubusercontent.com/cloudflare/pingora/main/tinyufo/README.md)

### 2.4 对 Edgion 的启发

如果 Edgion 后续要做 cache，Pingora 生态可以拆成两类借鉴：

- 做“HTTP 内容缓存”时，借鉴 `pingora_cache`
- 做“节点内热点对象缓存 / stampede 防护”时，借鉴 `pingora-memory-cache + TinyUFO`

不要混用：

- `pingora_cache` 解决的是代理内容缓存语义
- `pingora-memory-cache` 更像可复用的高性能内存 cache 组件

## 3. Nginx 类 proxy cache

### 3.1 核心机制

Nginx `ngx_http_proxy_module` 官方文档给出的关键信号非常完整：

- `proxy_cache`
- `proxy_cache_key`
- `proxy_cache_lock`
- `proxy_cache_background_update`
- `proxy_cache_revalidate`
- `proxy_cache_use_stale`
- `proxy_cache_valid`
- `proxy_cache_path`

尤其关键的是：

- `proxy_cache_path` 说明 cache data 存在文件里，文件名由 cache key 的 MD5 生成
- 活跃 key 和元数据放在共享内存 zone 里
- `proxy_cache_lock` 让同一个 key 只有一个请求回源填充，其他请求等待或超时旁路
- `proxy_cache_background_update` 可在返回 stale 的同时后台刷新
- `proxy_cache_key` 明确 cache key 设计是第一等能力

这是一种非常典型的“两层结构”：

- 内存：key 索引/元数据
- 磁盘：响应体对象

参考：

- [NGINX ngx_http_proxy_module](https://nginx.org/en/docs/http/ngx_http_proxy_module.html)

### 3.2 Nginx 类 cache 的作用

它解决的问题主要是：

- 减少回源
- 降低热点对象放大
- 提供 stale 容灾
- 用磁盘扩展容量
- 为大对象与长尾对象提供比纯内存更便宜的存储

这和插件内部的 TTL key-value cache 不是同一类问题。

### 3.3 对 Edgion 的启发

如果 Edgion 将来做 `EdgionCache`：

- 更接近 Nginx `proxy_cache` / Pingora `HttpCache`
- 需要至少包含：
  - cache key 规则
  - cacheability 判断
  - storage 抽象（memory / disk / future object store）
  - cache lock
  - stale-if-error / stale-while-revalidate
  - purge / bypass / no-cache
  - range / method / vary 处理

## 4. Rust 常见开源 cache 实现

下面这些更适合做“节点内对象 cache / plugin cache”，不是完整 proxy content cache。

### 4.1 Moka

官方 docs.rs 说明：

- 高并发内存 cache
- 支持 sync / async
- 支持按条目数或权重限流
- 支持 TTL、TTI、per-entry expiration
- 驱逐策略基于 Caffeine，LFU 准入 + LRU 驱逐（TinyLFU）

适合：

- token/introspection 结果 cache
- config 派生对象 cache
- 短 TTL metadata cache

参考：

- [Moka docs.rs](https://docs.rs/moka/latest/moka/)

### 4.2 mini-moka

`mini-moka` 是 `Moka` 的轻量版。

特征：

- 并发内存 cache
- 也支持容量上限、权重、TTL、TTI
- 更轻、更适合对功能面要求不那么全的场景

适合：

- gateway 内部轻量 TTL cache
- 不需要复杂监听器和 async 特性的插件 cache

参考：

- [mini-moka docs.rs](https://docs.rs/mini-moka/latest/mini_moka/)

### 4.3 quick_cache

官方 docs.rs 说明：

- “Lightweight, high performance concurrent cache”
- 驱逐策略是改造版 `Clock-PRO`，很接近 `S3-FIFO`
- 提供 `get_or_insert` / `get_value_or_guard`，可协调 miss 期间只计算一次

适合：

- 高性能本地对象 cache
- 需要单飞/并发协调的插件结果 cache
- 比较适合作为 `ForwardAuth` / metadata 查询类插件的底层 cache

参考：

- [quick_cache docs.rs](https://docs.rs/quick_cache/latest/quick_cache/)

### 4.4 cached

`cached` 更偏函数 memoization 框架。

官方 docs.rs 说明：

- 提供缓存结构与函数 memoization 宏
- 支持 `#[cached]`、`#[once]`、`#[io_cached]`
- 可接 Redis 与 disk store
- 支持 `sync_writes`，但默认并不会在函数执行期间全程锁住 miss key

适合：

- 工具函数或小块逻辑的 memoization
- 配置派生计算缓存
- 不太适合作为 proxy 热路径的大规模核心 cache 基础设施

参考：

- [cached docs.rs](https://docs.rs/cached/latest/cached/)

### 4.5 lru

`lru` 是最朴素的本地 LRU cache。

官方 docs.rs 说明：

- `get/get_mut/put/pop` 都是 `O(1)`
- 单纯、清晰、可控

适合：

- 小规模本地缓存
- 逻辑简单、淘汰策略明确的场景

不太适合：

- 高并发 async 热路径
- 需要 TTL/TTI/单飞/权重/统计的复杂网关场景

参考：

- [lru docs.rs](https://docs.rs/lru/latest/lru/)

### 4.6 Stretto

官方 docs.rs 说明：

- Rust 版高性能 memory-bound cache
- 提供 sync/async 版本
- TinyLFU admission + Sampled LFU eviction

适合：

- 对吞吐和 hit ratio 很敏感的本地热点对象 cache
- 作为网关 L1 metadata cache 的候选

参考：

- [stretto docs.rs](https://docs.rs/stretto/latest/stretto/)

## 5. 面向 Edgion 的选型建议

### 5.1 如果目标是“上游响应缓存”

优先方向：

- `pingora_cache`
- 借鉴 Nginx `proxy_cache` 语义

因为这里最关键的是：

- HTTP 语义正确性
- cache key / vary / freshness / stale
- stampede 防护
- storage 抽象

而不是简单 KV cache。

### 5.2 如果目标是“插件内部短 TTL cache”

优先方向：

- 轻量：`quick_cache` / `mini-moka`
- 功能全：`moka`
- 极简：`lru`

建议场景：

- `ForwardAuth`
- `OpenidConnect` 补统一 cache 组件
- `AllEndpointStatus`
- 动态 metadata / 解析结果

### 5.3 如果目标是“多节点一致的共享状态”

优先方向：

- Redis
- Etcd（更适合协调/锁，不太适合高频计数）

适合：

- cluster-wide rate limit
- 分布式锁
- 配额共享

不建议用本地内存 cache 伪装成共享状态。

### 5.4 如果目标是“body 相关插件能力”

优先方向不是 KV cache，而是：

- request body buffer/spool 层
- 小对象内存，大对象落盘
- 可重复读取
- 上限 + 背压 + 生命周期清理

## Final Recommendation

从 Edgion 的现状看，后续最好分三条线推进，而不是一条线：

1. `EdgionCache`：面向 HTTP response content cache
2. `PluginLocalCache`：面向插件内部 TTL / matcher / result cache
3. `BodyStore`：面向请求体缓存与重读

如果只做一个“通用 cache 抽象”，大概率会把三类完全不同的正确性问题缠在一起。

## Sources

- [Pingora 仓库 README](https://github.com/cloudflare/pingora)
- [pingora_cache docs.rs](https://docs.rs/pingora-cache/latest/pingora_cache/)
- [pingora-memory-cache docs.rs](https://docs.rs/pingora-memory-cache/latest/pingora_memory_cache/)
- [TinyUFO README](https://raw.githubusercontent.com/cloudflare/pingora/main/tinyufo/README.md)
- [NGINX ngx_http_proxy_module](https://nginx.org/en/docs/http/ngx_http_proxy_module.html)
- [Moka docs.rs](https://docs.rs/moka/latest/moka/)
- [mini-moka docs.rs](https://docs.rs/mini-moka/latest/mini_moka/)
- [quick_cache docs.rs](https://docs.rs/quick_cache/latest/quick_cache/)
- [cached docs.rs](https://docs.rs/cached/latest/cached/)
- [lru docs.rs](https://docs.rs/lru/latest/lru/)
- [stretto docs.rs](https://docs.rs/stretto/latest/stretto/)

## Risks

- 外部 cache 库适合“节点内对象缓存”并不等于适合“代理响应缓存”
- 直接照搬 Nginx 语义但不处理 Edgion 现有 route/plugin 生命周期，容易把失效链做乱
- 如果未来内容缓存与认证插件缓存共用同一层统计/驱逐，会很难调优
