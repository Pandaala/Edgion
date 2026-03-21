# Step 01 - Design

## Decision

第一版不照搬 OpenResty worker-local 模型，也不直接引入 `pingora-memory-cache` 或 `mini-moka`。

第一版实现为：

- 单进程内共享
- 使用普通互斥锁
- `HashMap + VecDeque`
- 精确本地 LRU
- 惰性 TTL 过期

## Why

这版目标是最小可用，不是最终高并发最优解。

需要优先满足：

- 方便插件接入
- 容易审查
- 容易调试
- 容易后续替换为更复杂实现

## API Boundary

建议支持：

- `new(capacity)`
- `insert(key, value, ttl)`
- `get(key) -> Option<V>`
- `get_with_status(key) -> (Option<V>, CacheStatus)`
- `remove(key)`
- `clear()`
- `len()`
- `is_empty()`

## Concurrency Model

- 用 `parking_lot::Mutex`
- `get` 也持锁，因为命中后需要更新 LRU 顺序
- 容量不做分片，不做无锁

## Risks

- `VecDeque` 里线性删除是 `O(n)`，容量过大时不适合热点高并发
- 但第一版定位是插件短 TTL 结果缓存，容量可控，复杂度优先

## Next

- 实现基础模块
- 先通过单元测试验证 TTL / LRU / 更新行为

