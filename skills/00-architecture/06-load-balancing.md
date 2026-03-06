# 负载均衡

> Edgion 负载均衡架构：基于 `pingora-load-balancing` 的多策略支持，包括后端发现、健康检查和 LB 选择器。

## LB 模块结构

```
src/core/gateway/lb/
├── backend_selector/
│   ├── selector.rs          # BackendSelector trait + 公共入口
│   └── weighted_selector.rs # 加权选择器
├── ewma/                    # EWMA (Exponentially Weighted Moving Average) 策略
├── leastconn/               # 最少连接策略
├── lb_policy/               # LB policy 解析与存储
└── mod.rs
```

## 支持的负载均衡策略

| 策略 | 说明 | 适用场景 |
|------|------|---------|
| **RoundRobin** | 轮询（pingora 内置） | 默认策略，后端性能一致 |
| **Random** | 随机选择（pingora 内置） | 简单场景 |
| **EWMA** | 指数加权移动平均延迟 | 后端性能不一致，自动偏向快节点 |
| **LeastConn** | 最少活跃连接 | 长连接场景 |
| **ConsistentHash** | 一致性哈希（基于请求属性） | 需要会话亲和性 |
| **WeightedSelector** | 加权选择 | 灰度发布、流量分配 |

## 后端发现

```
src/core/gateway/backends/
├── discovery/
│   ├── services/        # Service 资源处理
│   ├── endpoint_slice/  # EndpointSlice 资源处理
│   └── endpoint/        # Endpoint 资源处理（兼容旧版）
├── health/
│   └── check/           # 健康检查配置、探测器、状态存储
└── policy/
    └── backend_tls/     # BackendTLSPolicy 存储与处理
```

**两种 Endpoint 模式**（由 Controller `endpoint_mode` 决定）：
- `EndpointSlice` — K8s 1.21+ 推荐，支持大规模集群
- `Endpoint` — 兼容旧版 K8s

后端信息通过 gRPC 从 Controller 同步到 Gateway，Gateway 侧的 discovery stores 维护运行时后端列表，health 子层负责健康检查状态筛选，policy 子层负责 BackendTLSPolicy。

## LB 选择流程

```
upstream_peer() hook in ProxyHttp
  │
  ├─ HTTP Route:
  │   ctx.route_unit → selected_backend → BackendRef
  │   → resolve service → get backends
  │   → LB strategy select (with health check filter)
  │   → return HttpPeer (ip:port + TLS config)
  │
  └─ gRPC Route:
      ctx.grpc_route_unit → same flow
      → return HttpPeer with h2 flag
```

## 健康检查

基于 `pingora_load_balancing::Backend` 的健康状态，不健康后端自动从选择池中剔除。
后端恢复后自动加回。

## 通过 Gateway API 配置 LB

HTTPRoute 的 `backendRef` 支持通过 `ExtensionRef` 指定 LB 策略：

```yaml
filters:
  - type: ExtensionRef
    extensionRef:
      group: edgion.io
      kind: LoadBalancer
      name: ewma
```

## Key Files

- `src/core/gateway/lb/` — 所有 LB 策略实现
- `src/core/gateway/backends/discovery/` — 后端发现与运行时存储
- `src/core/gateway/backends/health/check/` — 健康检查配置与状态
- `src/core/gateway/backends/policy/backend_tls/` — BackendTLSPolicy
- `src/core/gateway/routes/http/proxy_http/pg_upstream_peer.rs` — LB 选择入口
