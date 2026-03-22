# Traefik vs Edgion 本地缓存竞争分析

**日期：** 2026-03-22
**分析对象：** Traefik `mailgun/ttlmap` + `singleflight` vs Edgion `LocalLruCache`
**数据来源：** Traefik master 源码、mailgun/ttlmap 源码、GitHub PR #7900 及 Issue #7897 讨论全文

---

## Part A — 技术深度分析

### A1. `mailgun/ttlmap` 内部结构

#### 数据结构

```go
type TtlMap struct {
    capacity    int
    elements    map[string]*mapElement   // 主索引 map
    expiryTimes *minheap.MinHeap         // 按到期时间排列的最小堆
    clock       timetools.TimeProvider
    mutex       *sync.RWMutex            // 可选并发锁（仅 NewConcurrent 时启用）
    onExpire    Callback
}

type mapElement struct {
    key    string
    value  interface{}
    heapEl *minheap.Element              // 指向堆中对应节点
}
```

核心是 **map + minheap 双结构**：map 提供 O(1) key 查找，minheap 持续追踪最近将过期的条目。

#### TTL 清理机制

TTLMap 没有后台 goroutine。**过期完全是惰性触发（lazy expiry）**，发生在两个时机：

1. **`Get` 时检查：** `lockNGet` 用 RLock 取值，若发现 `heapEl.Priority <= now` 则通过 `lockNDel` 升级为写锁后删除。
2. **`Set` 时触发 `freeSpace`：** 容量满时调用 `freeSpace(1)`，先尝试 `removeExpired`（弹出堆顶已过期元素），仍不足则调用 `removeLastUsed`——即**弹出堆中优先级最小的元素**，与 LRU 无关，是"最快到期的条目"被驱逐。

```go
func (m *TtlMap) removeLastUsed(iterations int) {
    for i := 0; i < iterations; i += 1 {
        heapEl := m.expiryTimes.PopEl()    // 弹出到期时间最早的元素
        mapEl := heapEl.Value.(*mapElement)
        delete(m.elements, mapEl.key)
    }
}
```

**注意：** 函数名为 `removeLastUsed`，实际语义是"最快到期"而非"最久未使用"，**命名误导性强**。

#### 并发模型

`NewConcurrent` 仅增加一个 `sync.RWMutex`：

- **读操作（`Get`）：** RLock → 读取值，若过期则升级到 Lock 删除
- **写操作（`Set`, `Increment`）：** 全程 Lock

锁粒度是整个 map，与 Edgion 的 `parking_lot::Mutex` 一样是单锁。

#### 容量控制

通过 `capacity` 字段做硬限制。`Set` 时若 `len(elements) >= capacity` 则触发 `freeSpace`。`maxSources` 在 Traefik 中硬编码为常量（rate limiter 文件中：`ttlmap.NewConcurrent(maxSources)`）。

#### ttlmap 缺少的能力

| 缺失能力 | 说明 |
|---------|------|
| **真正的 LRU 顺序** | 驱逐基于到期时间（最小堆顶），不是访问频率/时序 |
| **命中/未命中统计** | 无任何 metrics 接口 |
| **Introspection / get_keys** | 无法遍历当前活跃 key |
| **TTL 为 Duration** | TTL 单位是 `int` 秒，精度粗，不支持毫秒 |
| **泛型/类型安全** | Go interface{}，类型断言在调用方 |
| **驱逐回调区分过期 vs LRU** | onExpire 回调不区分两种驱逐原因 |

---

### A2. Traefik Basic Auth PR #7900 拒绝分析

#### 提案内容

PR #7900（作者：@wbolster，2021-02-16，已关闭未合并）提出：

> 在 BasicAuth 中引入小型内存 LRU Cache，缓存 `goauth.CheckSecret()` 的结果（bcrypt 验证结果），key 为 `SHA512(realm + user + password)`，大小硬编码为 128 条。

**动机：** bcrypt 的计算成本随 cost 参数指数级增长：

```
complexity 12:  0.17s
complexity 13:  0.35s
complexity 14:  0.68s
complexity 15:  1.37s
complexity 16:  2.71s
complexity 17:  5.42s
```

每次请求都触发 bcrypt 验证，对 Traefik Dashboard 这类"每几秒自动刷新"的场景，**结果是让整个服务器 CPU 饱和、所有路由延迟飙升**。

#### 拒绝理由（Traefik 维护者 @SantoDE 原话）

> - What would be the **strategy to revoke users**? (the cache lib you used is basically a fifo stack configured with 128 slots)
> - A fifo stack won't be beneficial in any way if it's **always full of unsuccessful attempts because somebody is trying to brute force the auth**
> - What is the impact on performances, making the **key to store in cache (sha512) versus computing bcrypt** each times
> - What are the **most common scenario of complexity** used with bcrypt encoded passwords?
> - In what cases does that PR **improve performance**?
> - What is the benefit for **high frequency configuration reload** context
> - We are also unsure, if that would not make Traefik sensitive to **time pad attacks**

#### Traefik 最终选择：`singleflight`

当前 basic_auth.go（master）的实际实现：

```go
type basicAuth struct {
    // ...
    checkSecret       func(password, secret string) bool
    singleflightGroup *singleflight.Group
}

func (b *basicAuth) checkPassword(user, password string) bool {
    secret := b.auth.Secrets(user, b.auth.Realm)
    if secret == "" {
        return false
    }
    key := password + secret
    match, _, _ := b.singleflightGroup.Do(key, func() (any, error) {
        return b.checkSecret(password, secret), nil
    })
    return match.(bool)
}
```

`singleflight.Do` 的语义是：**同一 key 的并发调用，只执行一次实际计算，其余调用等待并共享结果。计算完成后立刻释放，不持久化缓存**。

这解决了"同一用户在极短时间内并发请求导致的 N 倍 bcrypt 计算"问题，但不解决"同一用户每次新请求都重新计算"的问题。

#### 安全本质：bcrypt 的慢是有意为之

PR 被拒绝背后有一个更深的安全逻辑：

> **bcrypt 本身就是 Traefik 的防暴破屏障。** bcrypt 的高 cost 不只是密码存储的安全性，对攻击者来说每次尝试都很慢，这是 bcrypt 作为"计算限速器"的设计用意。

如果加了结果缓存：
1. **撤销窗口（Revocation window）：** 用户密码改变后，缓存仍持有旧的认证结果，TTL 内攻击者可继续访问
2. **暴破放大（Brute-force amplification）：** 攻击者用大量不同凭据持续请求，不仅打满 cache slots，还将所有 bcrypt 计算结果持久化在内存中
3. **时序侧信道（Timing side-channel）：** cache hit 极快，cache miss 极慢，攻击者可通过响应时间推断 cache 状态

Issue #7897 最终被 Traefik 官方以"没有足够关注度"为由关闭（2023），社区强烈反弹后重新开放，截至本文写作仍未解决。

---

### A3. `mailgun/ttlmap` vs `LocalLruCache` 功能对比

| 功能 | `mailgun/ttlmap` | `LocalLruCache` (Edgion) |
|-----|-----------------|--------------------------|
| **驱逐策略** | 最早到期（minheap 弹堆顶） | **LRU（双链表）+ TTL** |
| **per-entry TTL** | 是（int 秒精度） | 是（Duration 精度，支持毫秒） |
| **惰性 vs 后台清理** | 惰性（Get/Set 触发） | 惰性（insert 时 `prune_expired_from_tail`） |
| **容量上限** | 是（maxSources 硬编码） | 是（fixed node pool，预分配） |
| **并发安全** | `sync.RWMutex`（读共享写独占） | `parking_lot::Mutex`（单锁，但无读写分离开销） |
| **命中/未命中统计** | **无** | **有** (`CacheStats`：hits/misses/expirations/evictions) |
| **Key 遍历** | **无** | **有** (`get_keys()`，MRU 顺序) |
| **驱逐回调** | 有（`onExpire`，不区分原因） | 无（但 evictions 计数器可感知） |
| **内存分配** | 每次 Set 堆分配 mapElement | 预分配 node pool，零运行时分配 |
| **泛型/类型安全** | `interface{}`，调用方需类型断言 | Rust 泛型 `<K, V>`，编译期类型安全 |
| **TTL=0 语义** | 报错（ttlSeconds 必须 > 0） | 视为立即删除（`remove` 调用） |
| **语言** | Go | Rust |

**关键差异总结：**

- ttlmap 的"LRU"其实是"最快到期驱逐"，与真正 LRU（最久未使用）完全不同。如果有一个 TTL 很短但访问非常频繁的 entry，ttlmap 会驱逐它；LocalLruCache 会保留它。
- LocalLruCache 有可观测性（stats），ttlmap 是完全黑盒。
- LocalLruCache 的 node pool 预分配避免了 GC 压力（对 Rust 来说即 allocator 调用减少）。

---

## Part B — 竞争维度对比

### 维度一：限流缓存（Rate Limiting Cache）

| | Traefik | Edgion | 差距分析 |
|--|---------|--------|---------|
| **实现** | `mailgun/ttlmap`：map + minheap，按 per-source TTL 存储 `rate.Limiter` 桶 | 尚未基于 `LocalLruCache`；当前无专用限流插件 | Traefik 限流缓存已生产验证；Edgion 若实现限流可直接用 `LocalLruCache`，能获得真 LRU 驱逐、更好的热 key 保留 |
| **驱逐语义** | 最快到期（非热度感知） | LRU（热 key 优先保留） | Edgion **有优势**：高活跃 source 的 limiter 桶不会因 TTL 短而被意外驱逐 |
| **可观测性** | 无统计 | hits/misses/evictions 全部可见 | Edgion **有优势** |

### 维度二：认证结果缓存（Auth Result Cache）

| | Traefik | Edgion | 差距分析 |
|--|---------|--------|---------|
| **Basic Auth** | **故意不缓存**，仅用 `singleflight` 去重并发请求，无持久化 | BasicAuth 用 `DashMap` 自制缓存（仅缓存成功结果） | Edgion **功能上更强**，但 Traefik 的拒绝理由值得认真对待 |
| **LDAP Auth** | 无内置 LDAP 插件 | 用 `DashMap` 缓存 SHA-256(username:password) → 认证结果 | Edgion **独有能力** |
| **OIDC** | 无缓存，每次请求验证 JWT 签名（JWKS fetch 有本地 TTL state） | JWKS 用 `RwLock<JwksState>` 本地缓存，token 验证本地完成 | 两者类似，Edgion 的 `LocalLruCache` 可进一步优化 token 级缓存 |

### 维度三：设计哲学（Design Philosophy）

| | Traefik | Edgion | 差距分析 |
|--|---------|--------|---------|
| **auth 缓存立场** | **安全优先，故意不缓存 auth 结果**，接受 CPU 高作为可接受代价 | 可配置 TTL，让运营者决定 | Traefik 更保守；Edgion 更灵活，但需要清晰的文档和默认配置保护用户 |
| **缓存基础设施** | 依赖三方库（mailgun/ttlmap），无内置 LRU | 自研 `LocalLruCache`，功能更完整 | Edgion **有优势**：自研可控，zero 外部依赖 |
| **可观测性** | 无缓存 metrics | CacheStats 内置 | Edgion **有优势** |
| **社区历史** | Issue #7897 开了 3 年，官方至今未修复 bcrypt 性能问题 | Edgion 通过缓存直接解决了 Traefik 3 年未解决的问题 | Edgion **差异化优势** |

---

## Part C — 插件逐一决策指引

### ForwardAuth

**Traefik 等效插件：** `forwardAuth` middleware（完全对等）

**Traefik 的缓存行为：**
Traefik forwardAuth **没有任何缓存**，每次请求都完整调用外部认证服务，无内置 deduplication（连 singleflight 都没有）。

**Edgion LocalLruCache 能带来什么：**

ForwardAuth 的瓶颈是外部 HTTP 请求延迟（通常 5-100ms）。如果对同一 Authorization token 或 Cookie，短时间内有大量并发请求（典型场景：SPA 应用首次加载时并发 20-50 个资源请求），`LocalLruCache` 可以将多数请求短路到缓存命中，消除外部调用。

建议 cache key 设计：`SHA256(Authorization header value)` 或 `SHA256(Cookie value)`，TTL 建议 10-60 秒（与业务 session 有效期对齐）。

**Traefik 经验中 Edgion 应防范的风险：**

1. **凭据撤销窗口**：外部 auth service 可能已撤销 token（例如用户登出），但缓存仍命中。需提供强制失效 API（`/cache/invalidate`）或保持较短 TTL（≤30s）。
2. **缓存 403/401**：PR #7900 讨论中提到缓存"失败结果"也有意义（防止暴破打满 CPU），但会导致"重试失败"体验差。建议**仅缓存成功（2xx）结果**，失败请求不缓存（或 TTL 极短如 5s）。
3. **DoS 放大**：如果攻击者知道缓存存在，可以提前"预热"大量合法 token 占满 cache slots，然后注入恶意请求。`LocalLruCache` 的 LRU 驱逐可一定程度对抗这种攻击（热 key 不被挤出），比 ttlmap 的"最早到期驱逐"安全性更好。

---

### LdapAuth

**Traefik 等效插件：** 无（Traefik 无内置 LDAP 插件，需借助 forwardAuth 指向外部 LDAP 服务）

**Traefik 的缓存行为：** 不适用（无内置实现）

**Edgion LocalLruCache 能带来什么：**

LDAP bind 操作通常需要 10-100ms（网络 RTT + LDAP 服务处理），Edgion 当前用 `DashMap<String, (String, Instant)>` 缓存结果。

升级到 `LocalLruCache` 的优势：
- **容量控制**：`DashMap` 无容量上限，在暴破场景下会无限增长；`LocalLruCache` 固定 capacity 有天然防护。
- **LRU 驱逐**：活跃用户的认证结果不会被挤出；不活跃用户/攻击者尝试的失败结果自然衰退。
- **统计可见**：通过 `CacheStats` 可以检测到异常（命中率突然下降 = 可能有暴破攻击）。

**Traefik 经验中 Edgion 应防范的风险：**

1. **LDAP 账户被禁用/密码变更**：同 ForwardAuth 的撤销窗口问题。对 LDAP 来说更严重——企业安全团队要求"立即禁用账户"，缓存 TTL 内无法保证。建议提供 `ldap_auth.cache_ttl` 默认值 ≤60s，并在文档中明确说明撤销延迟。
2. **缓存失败结果的安全争议**：Traefik PR #7900 中提到，缓存失败结果可防止同一错误密码被反复尝试；但对 LDAP 来说，缓存失败结果会导致密码刚修改后的"短暂锁定"体验。**建议：仅缓存成功结果，失败结果不缓存。**

---

### OpenidConnect

**Traefik 等效插件：** Traefik Enterprise 的 `plugin-oidc`（闭源），开源版无内置 OIDC

**Traefik 的缓存行为：** 开源版无；Enterprise 版实现未知

**Edgion LocalLruCache 能带来什么：**

Edgion OIDC 插件目前有两层缓存：
1. **JWKS 缓存**（`RwLock<JwksState>`）：缓存整个 JWKS 集合，有 `expires_at` 字段
2. **Session 层**（Cookie + token）：session 本身作为 token 载体

**`LocalLruCache` 的潜在应用点：**

- **Token 黑名单（revocation check）**：已撤销的 token 放入 `LocalLruCache<String, bool>`（key = JTI），TTL 与 token 有效期对齐，避免每次请求访问 revocation endpoint。
- **Introspection 结果缓存**：对 opaque token 的 introspection 结果（`/introspect`）进行缓存，key = `SHA256(token)`，TTL = token 剩余有效期（上限 300s），消除高频 introspection 请求。
- **Discovery Document 缓存**：当前 `DiscoveryDocument` 在初始化时获取，可考虑周期性刷新并用 `LocalLruCache` 缓存中间态。

**Traefik 经验中 Edgion 应防范的风险：**

1. **JWKS key rotation**：OIDC provider 可能轮换密钥，旧 `kid` 的 JWK 被移除。Edgion 当前 `JwksState.expires_at` 控制刷新，但如果 token 验证失败（unknown kid），应立即触发 JWKS 刷新而不是等到过期——这是 OIDC 生产部署的经典坑。
2. **Replay attack vs cache hit**：缓存 token 验证结果时，如果攻击者窃取了有效 token，缓存会持续命中直到 TTL 过期；加入 `LocalLruCache` 的 revocation 缓存时需确保 revocation 查询本身的新鲜度。

---

## 最终结论

### Edgion 的具体竞争优势

1. **真 LRU 驱逐语义**：ttlmap 的"removeLastUsed"实际是"最快到期驱逐"，Edgion 的 `LocalLruCache` 是真正基于访问时序的 LRU，热 key 得到保护。

2. **可观测性领先**：Traefik 的 ttlmap 是完全黑盒；`LocalLruCache` 内置 `CacheStats`（hits/misses/expirations/evictions），可接入 Prometheus 暴露 cache hit ratio，用于告警和调优。

3. **解决了 Traefik 3 年未修复的问题**：Issue #7897 显示 Traefik 的 bcrypt 性能问题困扰了大量用户，官方至今未给出完整解决方案。Edgion 的认证结果缓存直接解决了这个问题，是明确的差异化卖点。

4. **内存分配可预测**：node pool 预分配，运行时零堆分配，比 ttlmap 每次 Set 分配 `mapElement` + heap 节点更适合低延迟场景。

5. **类型安全**：Rust 泛型 vs Go `interface{}`，编译期保证缓存值类型，运行时无 panic 风险。

---

### Traefik 做对了哪些值得 Edgion 学习

1. **`singleflight` 的精准使用**：singleflight 解决的是"并发重复计算"问题，不产生持久化缓存，没有撤销窗口风险，是安全性与性能的极佳平衡点。Edgion 在 LDAP 和 OIDC 的高并发场景下可考虑叠加 singleflight（在 `LocalLruCache` miss 时，对相同 key 的并发 LDAP bind / token introspection 请求做 dedup）。

2. **对 bcrypt 缓存安全顾虑的严肃态度**：Traefik 维护者提出的撤销窗口、暴破放大、时序侧信道等问题全部合理。Edgion 的 auth 缓存应当：
   - 默认仅缓存成功结果
   - TTL 短且可配置，文档中明确说明撤销延迟
   - 建议管理员配合速率限制使用（不能单独依赖缓存来防暴破）

3. **maxSources 硬编码容量**：Traefik 用常量限制 ttlmap 大小，防止内存无限增长。Edgion 的 `LocalLruCache` 也采用了固定容量，这是正确的。

---

### 安全考量（PR #7900 的核心教训）

**bcrypt 的慢是防御，不是 bug。**

这是 PR #7900 讨论中最重要的一句话。bcrypt 的高 cost 参数：
- 阻止暴破（攻击者每次尝试也慢）
- 增加密码泄露后的破解成本

如果 Edgion 为 BasicAuth 加了 LRU 缓存，实际上把 bcrypt 的防暴破屏障降低到"第一次验证成功后，后续 TTL 内无论多少次请求都零成本"。这意味着：

**攻击者的最优策略变成：**
1. 成功验证一次（获取 cache entry）
2. 在 TTL 窗口内以最大速度发出请求（每次都命中缓存，几乎无 CPU 成本）

**建议的 Edgion 缓存安全规则：**

```
缓存应用原则（按安全级别）：
1. ForwardAuth：可缓存，外部服务已有完整 authz 逻辑，TTL ≤ 60s
2. LdapAuth：可缓存成功结果，TTL ≤ 60s，失败不缓存
3. BasicAuth (bcrypt)：谨慎，TTL 建议 ≤ 30s，需配合 IP 限速；
                        或只用 singleflight（不持久化缓存）
4. OIDC（本地 JWT 验证）：无需结果缓存，JWKS 缓存即可，本地验证本身极快
5. OIDC（introspection）：可缓存，TTL = min(token_exp - now, 300s)
```

最终一句话：**Traefik 没有做错，他们在用 singleflight 保守地解决并发问题。Edgion 的 LocalLruCache 提供了更强的性能工具，但必须配合明确的 TTL 策略、单独的速率限制、和诚实的文档，才能在竞争中真正赢得企业客户的信任。**
