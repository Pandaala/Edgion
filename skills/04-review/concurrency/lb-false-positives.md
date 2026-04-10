# LB 模块常见误判场景

> 来源：2026-04 LB 模块 Review，逐一对照源码核实后的结论。

---

## BackendSelector ArcSwap 初始化 TOCTOU 不是实际问题

**误判场景**：`BackendSelector::init` 使用 `load` 判断后 `store`，与调用方 `is_initialized()` + `init()` 组合形成 TOCTOU，多线程可重复执行 `create_state` 并覆盖写入。

**实际情况**：

1. **输入恒等**：所有并发 `init` 的数据来源是同一个 `Arc<HTTPRouteRule>` 的不可变字段 `backend_refs`，输入完全一致，`create_state` 产出逻辑等价。
2. **整体替换**：配置更新时创建全新 `RouteRules`（含新 `BackendSelector`），不存在"用不同参数 reinit 同一 selector"的场景。
3. **窗口极小**：竞态仅在路由首次被请求时触发，之后 `is_initialized()` 直接返回 true，走快速路径。额外开销仅为几次 clone + 一次多余 store。
4. **内存安全**：`ArcSwap` 保证原子性，无数据撕裂或 UAF。

**判定**：理论上是经典 TOCTOU 模式，但结合项目约束（同源输入、整体替换、一次性触发），无可观测的负面后果，不需要修复。若未来改为原地 reinit 或支持不同参数的二次初始化，再引入 CAS 或 `OnceLock`。

---

## ConsistentHashRing 不支持 Unix Domain Socket 不是实际问题

**误判场景**：`ConsistentHashRing::build` 和 `sp_affinity_build` 中 `filter_map` 仅处理 `SocketAddr::Inet`，`SocketAddr::Unix` 被静默丢弃，UDS 后端不参与一致性哈希和会话持久化。

**实际情况**：

1. **发现层只产生 Inet 地址**：Backend 地址来自 K8s Endpoint/EndpointSlice 的 IP:port，通过 `"ip:port".parse::<SocketAddr>()` 构造（`endpoint_slice/discovery_impl.rs`、`endpoint/discovery_impl.rs`），永远是 `Inet`。
2. **UDS 后端在当前架构中不可能出现**：没有任何发现/构造路径会生成 `SocketAddr::Unix` 的 `Backend`。`filter_map` 的 `else` 分支是针对 pingora 类型系统的穷尽匹配，实际是死代码。
3. **SP affinity 同理**：`sp_affinity_build` / `sp_affinity_get_or_build` 的 Inet-only 过滤不影响实际行为。

**判定**：类型系统层面的未覆盖分支，但运行时不可达。若未来引入 UDS 后端发现（如 sidecar 场景），需同步扩展 CH 和 SP 路径，届时自然会处理。当前不需要修复，也不需要加 warn 日志（会造成无意义噪声）。

---

## EWMA 整数运算精度损失不是实际问题

**误判场景**：`update_ewma` 使用百分制整数 alpha（0-100）和 `/ 100` 截断除法更新 `AtomicU64`，每步丢弃余数。初始值 1ms 与真实 RTT 脱节。全局 alpha 无法 per-service 配置。

**实际情况**：

1. **截断误差可忽略**：数值模拟显示，整数 vs 浮点的累积差异始终在 ~2µs 以内。EWMA 用于比较的后端延迟差异通常在 ms 级别，截断误差仅占信号的 ~0.05%，对选路决策无影响。
2. **初始值收敛快**：从 1ms 初始值到 50ms 真实 RTT，alpha=20 时约 10 次观测即收敛到 90%+。给新后端多发几个请求本质上是合理的探测行为。
3. **alpha 粒度足够**：EWMA 本身是启发式近似，1% 步长对负载均衡选路决策没有实际影响。
4. **全局 alpha 是设计选择**：per-service alpha 属功能增强，非 bug。

**判定**：实现正确，精度对负载均衡场景完全足够，不需要修复。

---

## Session Persistence CRC32 碰撞不是实际问题

**误判场景**：SP affinity map 用 CRC32 哈希 `ip:port` 作 token 键，2^32 空间存在生日悖论碰撞；`HashMap::insert` 静默覆盖导致后端丢失。另涉 token 安全性、缓存一致性、1024 硬编码上限等。

**实际情况**：

1. **碰撞概率极低**：典型 K8s Service 3-50 endpoints，n=50 时碰撞概率 2.85e-7（约 350 万个 Service 才碰一次）。即使达 1024 上限仅 0.012%。实测 1024 个结构化 K8s IP 零碰撞。
2. **Token 安全性足够**：CRC32 不可逆，cookie 已有 HttpOnly + SameSite=Lax。猜中 token 仅粘到某后端，不构成权限提升。网关领域标准做法。
3. **缓存一致性是共享模式**：SP_AFFINITY 与 RR_CACHE、CH_CACHE 共用 `invalidate_selector_cache`，由 endpoint discovery handler 触发，非 SP 特有问题。
4. **1024 上限合理**：K8s Service 极少超此数，属 nice-to-have。
5. **`sp_affinity_build` 死代码**：确认无调用方，P3 代码卫生，不影响正确性。

**判定**：设计合理，碰撞风险在实际规模下可忽略，不需要修复。

---

## PolicyStore remove_by_route 的 clone 不是性能问题

**误判场景**：`remove_by_route` 在 `DashMap::retain` 回调里对每条 `ServiceLbPolicy` 执行 `clone()` 再写回，引入与 store 规模成正比的深拷贝开销。`batch_remove_routes` 对每个路由重复全表扫描。

**实际情况**：

1. **控制面冷路径**：`remove_by_route` 仅在 HTTPRoute 删除/同步时调用，非每请求的数据面路径，频率极低。
2. **数据规模极小**：DashMap 大小 = 配置了 LB 策略的 Service 数（通常个位数到几十）。`ServiceLbPolicy` 仅含短 `Vec<LbPolicy>` + 小 `HashSet<String>`，clone 开销可忽略。
3. **`changed` 语义正确**：仅检测 policies 长度变化是合理的——`route_refs` 变化不改变实际 LB 行为，不需要触发 endpoint slice 更新。

**判定**：代码可以更简洁（直接原地修改而非 clone+写回），但在控制面冷路径+极小数据规模下性能影响为零，不需要修复。

---

## BackendCleaner 缺少优雅关闭不是实际问题

**误判场景**：`BackendCleaner::stop()` 仅设标志位不等待任务退出，`start()` 丢弃 JoinHandle，无法 await 优雅关闭。存在关闭顺序竞态和 panic 隔离缺失。

**实际情况**：

1. **`stop()` 从未被调用**：全仓库无调用方。cleaner 随进程生命周期存活，进程退出时 runtime drop 自然终止任务。
2. **关闭由 Pingora 管理**：优雅关闭走 Pingora Server 的 grace_period + graceful_shutdown_timeout，排空在途请求。cleaner 清理的是内存中的统计状态，进程退出后自动消失，无需显式清理。
3. **并发安全由 DashMap 保证**：cleaner 与 endpoint discovery 并发不会损坏数据，最多多/少做一次清理，下周期自动修正。
4. **JoinHandle 丢弃是标准模式**：fire-and-forget 后台任务与进程同生命周期，不保存 handle 是常见做法。

**判定**：cleaner 是进程级后台任务，无需独立于进程的优雅关闭语义，不需要修复。

---

## select_by_min_metric BinaryHeap 小规模性能不是问题

**误判场景**：`select_by_min_metric` 每次选路创建 BinaryHeap，对 3-10 个后端的小池子 heapify 开销高于线性扫描。

**实际情况**：每个后端需做 `is_backend_active`（DashMap lookup）+ `metric_fn`（DashMap lookup），两次并发 map 查找才是真正的时间开销。对 3-10 个元素 heapify vs 线性扫描的差异是纳秒级，完全被 DashMap 读取淹没。`with_capacity(n)` 对 n<10 分配几十字节，现代分配器接近 noop。

**判定**：瓶颈在 DashMap 查表而非数据结构选择，为微优化引入阈值分支反增复杂度，不需要修复。

---

## try_get_peer 重复代码已修复

**原问题**：`try_get_peer` 的 Service/ServiceEndpoint/ServiceEndpointSlice 三个 match 臂有 ~25 行重复的 upstream 填充 + peer 构建逻辑。

**修复**：提取 `build_lb_peer` 公共函数，三个 match 臂简化为"选 backend + 指定 PortLookupSource + 调用 build_lb_peer"。消除 ~60 行重复。注意：因 `lb_policy` 借用自 `ctx`，提取函数时需先 clone `lb_policy` 断开借用。

---

## select_from_backends 9 个参数不需要重构

**误判场景**：`select_from_backends` 有 9 个参数，其中 5 个是 EdgionStatus 错误码（source×algorithm 组合），建议传入 source 枚举并内部映射。

**实际情况**：只有 2 个调用方（`select_from_endpoint_slice`、`select_from_endpoints`），都在同文件紧邻定义。10 个错误码变体全仓库仅在 `backends/mod.rs` 使用，无外部消费。新增 LB 算法频率极低。重构为 source 枚举 + 映射函数只是把显式参数变成隐式映射，调试反而多一层。

**判定**：2 个调用方、同文件可见、扩展频率极低，不需要重构。
