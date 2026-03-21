# Step 01 - Source And Takeaways

## Source

源码目录：

- `tasks/working/openresty-lru-analysis/vendor/lua-resty-lrucache`

关键文件：

- `lib/resty/lrucache.lua`
- `lib/resty/lrucache/pureffi.lua`
- `README.markdown`

## What It Actually Is

README 的核心定义很清楚：

- 这是一个 Lua VM 内部的 LRU cache
- 支持 TTL
- 不跨 OS 进程共享
- 适合每个 Nginx worker 进程内本地缓存

这和 Edgion 的相同点是：

- 都适合“本地短 TTL 结果缓存”

不同点是：

- OpenResty 默认是 worker 进程内模型
- Edgion/Pingora 是单进程多线程 async 模型

所以：

- 思想上可以借鉴
- 并发实现不能照搬

## Main Implementation Pieces

### 1. 经典版 `resty.lrucache`

`lrucache.lua` 的核心结构非常直接：

- `hasht`: key -> value
- `key2node`: key -> queue node
- `node2key`: node -> key
- `free_queue`: 预分配空闲节点队列
- `cache_queue`: 实际缓存队列

关键行为：

- `new(size)` 时一次性预分配节点池
- `get(key)` 命中后把节点移动到队首，保持 MRU
- `set(key, value, ttl, flags)`：
  - 如果有空闲节点就复用
  - 没空闲节点就淘汰 LRU 队尾
- `delete(key)` 把节点放回 free queue

对应源码位置：

- 预分配和双队列：`lib/resty/lrucache.lua:155-170`
- `get` 命中触碰：`lib/resty/lrucache.lua:184-204`
- `set` 插入/淘汰：`lib/resty/lrucache.lua:227-260`

### 2. `pureffi` 版本

`pureffi.lua` 不是“另一种策略”，而是为了绕开 Lua table 的删除和 rehash 问题。

README 说得很直白：

- 当命中率低、key churn 很高、Lua table 频繁删除时
- 普通 Lua table 会在 `resizetab` 上变热
- 所以提供 FFI hash table 版本

`pureffi` 的结构更复杂：

- key/value vector
- 节点数组
- LRU 双向链表
- 冲突链式 hash bucket

源码自述位置：

- 设计说明：`lib/resty/lrucache/pureffi.lua:4-58`
- bucket size / load factor：`lib/resty/lrucache/pureffi.lua:315-320`

## What Edgion Should Borrow

### A. 值得借鉴的部分

#### 1. 本地 cache 的定位

这个最值得借：

- 它就是“进程内本地短 TTL cache”
- 不试图承担共享状态
- 不试图承担内容缓存

这和我们现在为 Edgion 定的第一阶段目标完全一致。

#### 2. 固定容量 + 明确 LRU 队列

OpenResty 用了很朴素但清晰的模型：

- 有上限
- 有 MRU/LRU 队列
- 命中就触碰
- 满了就淘汰

这套思路适合 Edgion 第一版。

#### 3. 预分配节点池 / free queue 思路

这是一个很好的后续优化方向：

- 避免反复分配/释放节点
- 容量固定时内存形状更稳定
- 淘汰时直接复用节点

对 Edgion 来说：

- 第一版不一定要马上做
- 但第二版如果想降低分配和 clone，可以往 `slab` / node pool 方向演进

#### 4. TTL 是 entry metadata，不单独做后台线程

OpenResty 的 TTL 很简单：

- entry 上记录 `expire`
- 读取时检查过期

这和我们现在的惰性过期方向是一致的。

#### 5. 调试/观测接口

OpenResty 提供了：

- `count()`
- `capacity()`
- `get_keys()`，而且是 MRU 顺序

这对 Edgion 也很有价值，后面可以考虑补：

- `keys_mru()`
- `stats()`

## What Edgion Should Not Copy

### B. 不要直接照搬的部分

#### 1. `pureffi` 整套实现

这部分几乎完全是 LuaJIT / Lua table 行为驱动出来的工程权衡。

Rust 里：

- `HashMap` 删除是真删除
- 不存在 Lua table 那种 key=nil 但物理桶迟迟不收缩的问题

所以 `pureffi` 的主要存在理由在 Rust 里并不成立。

结论：

- 不要借 `pureffi` 的自定义 hash table
- 不要借它的 CRC32 pointer hashing
- 不要借它的 FFI 节点数组和冲突链结构

#### 2. worker-local 的无锁直觉

OpenResty worker 内本地 cache 不需要面对 Rust 这边的多线程共享访问。

Edgion/Pingora 下：

- `get` 会更新 LRU
- 所以 `get` 不是纯读
- 本地 cache 也仍然要锁

所以不能把“OpenResty 里没显式加锁”理解成 Edgion 也可以不锁。

#### 3. expired item 的语义直接照搬

README 里有两个细节值得注意：

- `get` 会返回 `stale_data`
- `count()` 明确包含 expired items

这些语义对 OpenResty 是合理的，但 Edgion 不一定要完全跟随。

当前我更建议 Edgion 保持简单：

- `get` 返回 `Hit/Miss/Expired`
- 过期项尽量在访问或插入路径上清掉
- 不急着暴露 stale value

#### 4. `set` 时不先优先清 expired 的行为

从 `lrucache.lua` 的 `set` 实现看，它更偏固定 node 复用和纯 LRU 淘汰，并没有先做一轮 expired-prune 再决定淘汰谁。

这个点我们反而不该照搬。

对短 TTL cache 来说，更合理的是：

- 插入前先清理 expired
- 再决定是否淘汰有效项

我们当前的 Rust 版本已经按这个方向修过一次了。

## Recommendation For Edgion

### 第一阶段应该借的

- 本地 cache 定位
- 容量上限
- 命中即触碰
- TTL 作为 entry metadata
- 清晰的观测接口

### 第二阶段可以考虑借的

- 固定节点池 / free list
- 更少分配的队列结构
- `get_keys()` / debug dump

### 当前不该借的

- `pureffi`
- 自定义 hash table
- LuaJIT/FFI 相关所有实现技巧
- worker-local 无锁心智模型

## Final Judgment

OpenResty LRU 最值得 Edgion 借鉴的，不是它的底层技巧，而是它的边界定义：

- 本地
- 简单
- 只解决短 TTL 和容量淘汰

OpenResty 代码里真正对我们有工程价值的部分主要是：

1. 本地短 TTL cache 的定位
2. 明确的 LRU 触碰和淘汰语义
3. 节点池/free queue 这种后续优化方向

而不是：

1. LuaJIT FFI
2. 自定义 hash table
3. `pureffi` 的所有复杂技巧

## Risks

- 如果看到 OpenResty 代码“很轻”，就误以为 Edgion 也能无锁，这会把 Pingora 的并发模型看简单
- 如果后面急着做第二版优化，直接跳到 node pool/slab，代码复杂度会上升很快

