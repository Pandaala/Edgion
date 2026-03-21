# Step 01 - Source Shape And Fit

## Local Source

源码目录：

- `tasks/working/pingora-memory-cache-analysis/vendor/pingora`

## Adoption Signals

### Crates.io 指标

通过 crates.io API 能拿到这些基础信息：

- crate: `pingora-memory-cache`
- version: `0.8.0`
- total downloads: `22610`
- recent downloads: `6229`
- repository: `https://github.com/cloudflare/pingora`

下载量明显低于 `mini-moka`，但它属于更窄的网关/代理生态，不算异常。

### 公开反向依赖

reverse dependencies API 里至少能看到：

- `cf-oagw`
- `zentinel-proxy`
- `grapsus-proxy`

说明：

- 它不是只给 Pingora 主仓内部自用
- 但外部采用面目前明显没有 `mini-moka` 广

## Source Shape

### Pingora README 的定位

Pingora 仓库 README 直接把它描述为：

- `Pingora-memory-cache: Async in-memory caching with cache lock to prevent cache stampede`

这个定位和 Edgion 现在想要的“短 TTL 结果缓存”非常接近，尤其是：

- 异步
- 本地内存
- 防击穿

### 代码量

粗看相关源码总量：

- `pingora-memory-cache/src/*.rs` 约 `1185` 行
- `tinyufo/src/*.rs` 约 `1204` 行
- `pingora-timeout/src/*.rs` 约 `631` 行
- 合计约 `3020` 行

这比 `mini-moka` 的 `7500+` 行明显小。

但需要注意：

- 小的是“整套组合”
- 不是单一 crate 就足够自洽

### 依赖关系

`pingora-memory-cache` 的核心依赖有：

- `TinyUFO`
- `tokio`
- `async-trait`
- `pingora-error`
- `parking_lot`
- `pingora-timeout`

这说明：

- 如果你已经接受 Pingora 生态依赖，这个方向很自然
- 如果你想“把代码摘出来自己放进 Edgion”，还是要连 `tinyufo` 和一部分 timeout/错误语义一起看

## API And Behavior

### 1. `MemoryCache` 很贴近“短 TTL 结果缓存”

`pingora-memory-cache/src/lib.rs` 提供了一个非常直接的接口：

- `new(size)`
- `get`
- `get_stale`
- `put(key, value, ttl)`
- `remove`
- `multi_get`
- `multi_get_with_miss`

状态语义也很实用：

- `Hit`
- `Miss`
- `Expired`
- `LockHit`
- `Stale(Duration)`

这很适合做：

- auth result cache
- metadata cache
- stale-while-revalidate 风格的短时结果缓存

### 2. `RTCache` 自带 lookup coalescing

`read_through.rs` 的重点能力非常对路：

- miss 后自动回调异步 lookup
- 同 key 并发 lookup 合并
- lock age / lock timeout
- `get_stale`
- stale 返回后后台刷新

对 Edgion 来说，这尤其适合：

- `ForwardAuth`
- `OIDC introspection`
- DNS / metadata / external lookup

也就是说，它不是单纯“存进去再取出来”的 cache，而是已经带 read-through 模型。

### 3. 底层不是 LRU，而是 TinyUFO

`MemoryCache` 底层直接用：

- `TinyUfo<u64, Node<T>>`

而 `tinyufo` README/源码语义是：

- TinyLFU admission
- S3-FIFO eviction
- lock-free

这意味着它和 `mini-moka` 一样，都不是“纯 LRU”。

差别是：

- `mini-moka` 是更完整、更通用的并发 cache
- `pingora-memory-cache` 更像“为 async 网关场景收敛过的一套 cache + read-through”

## Comparison With Mini Moka

| 维度 | `pingora-memory-cache` | `mini-moka` |
|---|---|---|
| 定位 | 网关/代理风格本地 async cache | 通用高并发 cache |
| 代码量 | 相关组合约 `3020` 行 | 约 `7500+` 行 |
| 生态广度 | 较窄 | 更广 |
| TTL 支持 | 有 | 有 |
| TTI 支持 | 没看到像 `mini-moka` 那样完整 | 有 |
| read-through | 原生支持 `RTCache` | 不以内建 read-through 为主 |
| 防击穿 | 原生 lock/coalescing | 不是它的主打接口 |
| 适合 Edgion 当前目标 | 很适合 | 也适合，但略偏重 |

## Judgment

### 1. 如果只做“短 TTL 结果缓存”，`pingora-memory-cache` 比 `mini-moka` 更贴当前目标

原因：

- 直接是 async 风格
- 有 TTL
- 有 stale 语义
- 有 lookup coalescing
- 有 lock timeout / lock age

这些都正好贴近网关插件的外部查询缓存。

### 2. 如果打算“摘代码自己用”，它也比 `mini-moka` 更现实

因为：

- `pingora-memory-cache` 本体 API 很收敛
- `tinyufo` 和 `pingora-timeout` 总量也不算太夸张
- 比 `mini-moka` 那种完整通用 cache 框架更容易局部借鉴

但仍然不建议轻易直接拷贝，原因没变：

- 你还是会变成维护一个私有 fork
- 后续 bugfix 和行为差异要自己兜底

### 3. 对 Edgion 的最实际推荐

如果现在三选一：

1. 自己实现最小本地 TTL cache
2. 直接用 `pingora-memory-cache`
3. 直接用 `mini-moka`

我会给这个顺序：

- 先选 `pingora-memory-cache`
- 其次自己实现一个最小版本
- 最后才是 `mini-moka`

原因：

- 你明确不太想再引陌生三方
- 你们本来就在 Pingora 生态里
- 当前需求不是“通用缓存平台”，而是“短 TTL 结果缓存”

## Recommendation

对 Edgion 当前阶段，最合适的路径是：

### 方案 A

直接引 `pingora-memory-cache`，先只封装一层很薄的 API，服务于：

- `ForwardAuth`
- `OpenidConnect` introspection
- `LDAP` / 外部认证短 TTL cache

### 方案 B

如果你仍然不想加依赖，就参照它的 API 形状自己做一个最小版：

- `get`
- `get_stale`
- `put(ttl)`
- `remove`
- 同 key 并发单飞

这时应该借的是：

- `RTCache` 的思路
- 不是把 `TinyUFO` 整套抄进去

## Current Issues

- 还没有对 `pingora-memory-cache` 的实际易用性做 PoC
- 还没看它在 Edgion 当前 Rust / Pingora 版本组合下会不会有额外兼容问题

## Risks

- 如果直接引入 `pingora-memory-cache`，仍然会增加一个新依赖面，只是这个依赖面属于 Pingora 工作区
- 如果自己照着 `RTCache` 仿一个，最容易遗漏的是并发细节与 stale/timeout 边界

