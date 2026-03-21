# Step 01 - Source And Adoption

## Local Source

源码已经下载到：

- `tasks/working/mini-moka-analysis/vendor/mini-moka`

## Adoption Signals

### Crates.io 指标

通过 crates.io API 能拿到这些元信息：

- crate: `mini-moka`
- max_version: `0.10.3`（API 返回值）
- total downloads: `5,137,999`
- recent downloads: `1,124,828`
- repository: `https://github.com/moka-rs/mini-moka`

这至少说明它不是冷门实验库。

### 公开可见的反向依赖示例

从 crates.io reverse dependencies API 前两页可以直接看到一些真实使用者：

- `apache/opendal`
- `serenity`
- `casbin`
- `static-web-server`
- `linera-service`
- `schema-registry-client`
- `trailbase`

其中比较容易识别、信号比较强的：

- `opendal`
- `serenity`
- `casbin`
- `static-web-server`

额外查到的部分仓库地址：

- `casbin` -> `https://github.com/casbin/casbin-rs`
- `tower-resilience-cache` -> `https://github.com/joshrotenberg/tower-resilience`
- `proxide` -> `https://github.com/Rantanen/proxide`
- `mrps` -> `https://github.com/pchchv/mrps`

## Source Shape

### 代码量

本地 `wc -l` 粗看，源码总量大约：

- `7516` 行

最大头模块包括：

- `src/sync/base_cache.rs` 约 `1380` 行
- `src/sync/cache.rs` 约 `1120` 行
- `src/unsync/cache.rs` 约 `1458` 行
- `src/common/deque.rs` 约 `773` 行
- `src/common/frequency_sketch.rs` 约 `392` 行

这已经不是“很薄的一层 TTL map 封装”了。

### 依赖面

`Cargo.toml` 显示其核心依赖包括：

- `crossbeam-channel`
- `crossbeam-utils`
- `smallvec`
- `tagptr`
- `triomphe`
- `dashmap`（sync feature 下）

参考：

- `vendor/mini-moka/Cargo.toml:17-33`

这说明：

- 直接引库的依赖面不算特别夸张
- 但如果想“挑代码自己用”，你不会只拷一个 LRU 文件就结束

### 核心设计不是“纯 LRU”

README 和源码都明确说明它不是单纯的 LRU：

- README 写明它受 Caffeine 启发
- admission 受 `LFU` 控制
- eviction 受 `LRU` 控制
- 支持 `TTL` / `TTI`
- 支持 size-aware eviction

参考：

- `vendor/mini-moka/README.md:11-19`
- `vendor/mini-moka/README.md:40-53`
- `vendor/mini-moka/src/lib.rs:7-30`

更关键的是它内部有：

- `frequency_sketch`
- `deques`
- `housekeeper`
- `read/write op channels`
- 并发 map + 过期时钟 + 淘汰策略

例如：

- `frequency_sketch.rs` 明确是 Count-Min Sketch / TinyLFU 风格
- `sync/base_cache.rs` 里可以看到 `DashMap + crossbeam channel + housekeeper + deques + sketch`

参考：

- `vendor/mini-moka/src/common/frequency_sketch.rs:1-67`
- `vendor/mini-moka/src/sync/base_cache.rs:1-37`
- `vendor/mini-moka/src/sync/base_cache.rs:79-107`

## Judgment

### 1. 它不是“代码量很少，可以随手摘一部分”的那种库

如果目标是：

- 一个简单短 TTL 结果缓存
- 容量上限
- 最多再带点惰性清理

那么 `mini-moka` 明显已经超出这个复杂度。

它解决的是：

- 并发
- TTL / TTI
- size-aware eviction
- 较好的 hit ratio
- 比较完整的 admission/eviction 策略

所以它更像“完整产品级本地 cache”，不是一个小工具模块。

### 2. 直接拷代码的性价比不高

原因：

- 模块间耦合明显，不是一两个文件就能拎出来
- 有现成依赖和内部抽象，摘取后还得自己重构
- 一旦你们后面继续修 bug，相当于自己养一个 fork
- 其中有一部分实现思路来自 Caffeine，虽然许可没问题，但维护成本仍然在你们自己这边

### 3. 值得借的是“设计”，不太值得直接拷“实现”

适合借的设计点：

- admission / eviction 分层思路
- TTL / TTI 同时支持
- 并发读写与 housekeeping 分离
- 频率草图而不是纯 LRU

不太适合当前 Edgion 直接照抄的点：

- 完整 frequency sketch
- 完整 deque / policy / housekeeper 架构
- 整个 sync cache 管线

因为你们当前目标只是：

- 短 TTL 结果缓存

这离 `mini-moka` 的完整能力集还差一大截。

## Recommendation For Edgion

如果只想先支持“短 TTL 结果缓存”，我建议优先级如下：

### 方案 A：自己做一个最小本地 TTL cache

适合现在：

- `ForwardAuth`
- `OIDC introspection/access token`
- `LDAP auth result`

建议只支持：

- `get`
- `insert(ttl)`
- `remove`
- `invalidate_all(optional)`
- `max_entries`
- 惰性过期

这样代码量能压得很低，也更符合你现在“别引太多新东西”的偏好。

### 方案 B：如果决定引库，直接用 `mini-moka`

适合：

- 你们希望少写并发/淘汰/TTL 基础设施
- 未来多个插件共用一套本地 cache 组件

不建议的方案：

- 把 `mini-moka` 拆碎后复制进 Edgion

### 方案 C：再看 Pingora 生态

如果你更在意生态一致性，可以再单独对比 `pingora-memory-cache`。

但从这次结论看：

- `mini-moka` 值得“直接用”
- 不太值得“拆开抄”

## Current Issues

- crates.io 的版本元信息和本地仓库版本号看起来不完全一致，需要后续真要引入时再确认锁定版本
- 目前还没做 `pingora-memory-cache` 的同维度源码对比

## Risks

- 如果为了避免依赖而直接拷 `mini-moka` 代码，维护成本很可能比直接加依赖更高
- 如果只做纯 LRU，又拿去承载高并发热点 key，后面可能还得补 singleflight / better eviction
- 如果现在就追求 `mini-moka` 级别完整能力，短期会把简单需求做重

