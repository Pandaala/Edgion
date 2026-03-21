---
name: gateway-load-balancing
description: 负载均衡策略：RoundRobin、Random、EWMA、LeastConn、ConsistentHash、WeightedSelector，以及后端选择流程。
---

# 负载均衡

> 负载均衡模块负责从后端列表中选择目标实例。
> 默认使用加权轮询（WeightedRoundRobin），可通过 HTTPRoute 的 ExtensionRef filter 指定 LoadBalancer 策略。

## 负载均衡策略

| 策略 | 类型 | 配置别名 | 说明 |
|------|------|----------|------|
| RoundRobin | 默认 | （默认，无需配置） | 加权轮询，通过 `WeightedRoundRobin<T>` 实现 |
| Random | 内置 | （随机选择） | 随机后端选择 |
| EWMA | `LbPolicy::Ewma` | `"ewma"` | 基于响应时间的指数加权移动平均值，优先选择延迟低的后端 |
| LeastConn | `LbPolicy::LeastConnection` | `"leastconn"`, `"least-connection"`, `"leastconnection"`, `"least_connection"` | 最少连接数，优先选择活跃连接最少的后端 |
| ConsistentHash | `LbPolicy::Consistent` | `"consistent"`, `"consistent-hash"`, `"ketama"` | 一致性哈希，支持按 header/cookie/IP/query param 等计算哈希 |
| WeightedSelector | — | — | `BackendSelector<T>` 实现的通用加权选择器，用于多 BackendRef 间的选择 |

## LB 策略解析

`LbPolicy` 枚举定义了三种非默认策略：

```rust
pub enum LbPolicy {
    Consistent,       // 一致性哈希
    LeastConnection,  // 最少连接
    Ewma,             // 指数加权移动平均
}
```

- 通过 `LbPolicy::parse()` 解析字符串配置
- 支持 `LbPolicy::parse_from_string()` 解析逗号分隔的多策略字符串
- 未配置时默认使用 RoundRobin

## EWMA 和 LeastConn 的选择算法

EWMA 和 LeastConn 共享 `select_by_min_metric()` 算法：

1. 构建最小堆（BinaryHeap + Reverse），按 metric 值排序
2. 跳过非活跃后端（通过 `runtime_state::is_backend_active()` 检查）
3. 从堆中依次弹出候选者，通过 `health_filter` 检查健康状态
4. 返回第一个通过健康检查的最低 metric 后端
5. 最多迭代 `max_iterations` 次

## BackendSelector

`BackendSelector<T>` 是通用的后端选择器，用于 Route 中多个 BackendRef 之间的加权选择：

```rust
pub struct BackendSelector<T> {
    state: ArcSwap<Option<SelectorState<T>>>,
}

enum SelectorState<T> {
    Error(u32),                    // 配置错误（无 BackendRef 或权重不一致）
    Single(T),                     // 单后端，直接返回
    Multiple(WeightedRoundRobin<T>), // 多后端，加权轮询
}
```

特点：
- 懒初始化：首次使用时通过 `init(items, weights)` 初始化
- 线程安全：内部使用 ArcSwap
- 权重验证：要么全部配置权重，要么全部不配置；权重为 0 的后端被过滤掉
- 错误码：`ERR_NO_BACKEND_REFS`(1) 和 `ERR_INCONSISTENT_WEIGHT`(2)

## 后端选择流程

```
HTTPRoute rule
  └── BackendRef[] (带可选 weight)
       │
       ├── BackendSelector.select()  →  选择目标 BackendRef
       │
       └── 解析 service_key (namespace/name)
            │
            ├── 从 EndpointSlice/Endpoint store 获取后端列表 (Backend[])
            │
            ├── 应用 LB 策略选择具体后端
            │   ├── 默认: RoundRobin (Pingora LoadBalancer)
            │   ├── ConsistentHash: 按请求特征哈希
            │   ├── LeastConn: 最少连接
            │   └── EWMA: 最低延迟
            │
            ├── 健康检查过滤 (Pingora Backend health)
            │
            └── 返回 HttpPeer (IP:Port + TLS 配置)
```

## LB 配置方式

通过 HTTPRoute/GRPCRoute 的 `ExtensionRef` filter 引用 `LoadBalancer` 类型资源：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
spec:
  rules:
    - backendRefs:
        - name: my-service
          port: 8080
      filters:
        - type: ExtensionRef
          extensionRef:
            group: gateway.edgion.io
            kind: LoadBalancer
            name: my-lb-policy
```

`ParsedLBPolicy` 在 BackendRef 的 `extension_info` 字段中解析后存储，支持：
- `ConsistentHash(ConsistentHashOn)` — 指定哈希源
- `LeastConn`
- `Ewma`

## LB Preload（预热）

`preload_load_balancers()` 在启动完成后（`wait_for_ready()` 之后）调用，减少首请求延迟：

1. 收集所有路由类型（HTTP/gRPC/TCP/UDP/TLS）中引用的 (service_key, lb_policy) 对
2. 使用 HashSet 去重
3. 对每个 key 调用 `get_or_create()` 预创建 RoundRobin LB 实例
4. LeastConn/EWMA/ConsistentHash 读取 RR 的 backend 列表，不需要单独预创建

## 目录布局

```
src/core/gateway/lb/
├── mod.rs                    # 模块导出
├── backend_selector/         # 通用后端选择器
│   ├── selector.rs           # BackendSelector<T> 实现
│   └── weighted_selector.rs  # WeightedRoundRobin<T> 实现
├── selection/                # 各策略实现
│   ├── round_robin.rs        # RoundRobin 选择
│   ├── consistent_hash.rs    # 一致性哈希选择
│   ├── ewma.rs               # EWMA 选择
│   └── least_conn.rs         # 最少连接选择
├── ewma/                     # EWMA 指标跟踪
│   └── metrics.rs            # EWMA 延迟指标
├── leastconn/                # LeastConn 状态管理
│   ├── backend_state.rs      # 后端连接状态
│   └── cleaner.rs            # 过期状态清理
├── lb_policy/                # LB 策略配置
│   ├── types.rs              # LbPolicy 枚举定义
│   ├── config.rs             # 策略配置解析
│   └── policy_store.rs       # 策略存储
└── runtime_state/            # 运行时后端状态
    └── mod.rs                # 后端活跃状态跟踪
```
