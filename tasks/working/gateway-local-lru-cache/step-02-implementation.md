# Step 02 - Implementation

## Implemented

已新增：

- `src/core/gateway/cache/mod.rs`
- `src/core/gateway/cache/lru.rs`

并在：

- `src/core/gateway/mod.rs`

中注册 `cache` 模块。

## Current API

`LocalLruCache<K, V>` 当前支持：

- `new(capacity)`
- `capacity()`
- `len()`
- `is_empty()`
- `clear()`
- `remove(&key)`
- `insert(key, value, ttl)`
- `get(&key)`
- `get_with_status(&key)`

另有：

- `CacheStatus::{Hit, Miss, Expired}`

## Internal Structure

- 单把 `parking_lot::Mutex`
- `HashMap<K, CacheEntry<V>>`
- `VecDeque<K>` 维护 LRU 顺序
- `get` 命中后触碰到队尾
- `insert` 超容量后淘汰队首
- TTL 为惰性过期

## Known Tradeoff

- 触碰 key 时会在线性扫描 `VecDeque`
- 因此第一版更适合小容量、短 TTL、插件结果缓存场景
- 不适合超大容量热点缓存

