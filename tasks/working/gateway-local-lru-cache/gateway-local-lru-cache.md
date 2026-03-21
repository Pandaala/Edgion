# Gateway Local LRU Cache

## Goal

在 `gateway` 下实现一个第一版本地并发 TTL/LRU cache，服务于插件短 TTL 结果缓存需求。

## Current Scope

- 新增 `src/core/gateway/cache/`
- 新增 `src/core/gateway/cache/lru.rs`
- 提供单进程内共享、单把普通锁保护的本地 cache
- 支持容量上限、TTL、命中触碰更新 LRU、过期惰性清理
- 暂不接入具体插件

## Steps

- `completed` `step-01-design.md`
- `completed` `step-02-implementation.md`
- `completed` `step-03-validation.md`

## Out Of Scope

- 第一版不做分布式共享
- 第一版不做请求体缓存
- 第一版不做 proxy content cache
- 第一版不做 singleflight / cache stampede 防护
- 第一版不追求无锁或近似淘汰算法

