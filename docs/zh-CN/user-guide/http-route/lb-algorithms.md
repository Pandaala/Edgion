# 负载均衡算法

> **🔌 Edgion 扩展**
>
> 选择非默认的负载均衡算法是 Edgion 的扩展功能，通过挂载到 Service 的 `EdgionBackendTrafficPolicy` 自定义资源的 `spec.loadBalancer` 字段实现；这不是标准 Gateway API 的一部分。

Edgion 将请求分发到后端的各个 endpoint。默认使用加权轮询；你可以为某个 **Service** 选择一致性哈希、最少连接或基于延迟感知的 EWMA 算法。

`EdgionBackendTrafficPolicy` 还承载了同一 Service 的[主动健康检查](./backends/health-check.md)与[异常点检测](./backends/circuit-breaker.md)；三者的完整 schema 参考见 [full schema reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md)（英文）。

## 支持的算法

| 算法 | 选择方式 | 适用场景 |
|------|----------|----------|
| RoundRobin（默认） | 跨健康 endpoint 的加权轮询 | 通用 HTTP 流量 |
| ConsistentHash | 将请求属性哈希到 Ketama 环 | 缓存亲和性、按 key 粘性路由 |
| LeastConn | 选择活跃连接数最少的 endpoint | 长连接（gRPC stream、WebSocket） |
| Ewma | 选择指数加权移动平均延迟最低的 endpoint | 延迟不均匀的异构后端 |

未配置策略、或 `loadBalancer.type: RoundRobin` 时，使用默认的加权轮询选择器。

## 配置方式

负载均衡策略声明在 `EdgionBackendTrafficPolicy` 资源的 `spec.loadBalancer` 字段中，通过 `spec.targetRefs` 挂载到 Service（策略与 Service 须在同一命名空间）。这是**按目的地**的配置：无论有多少条路由把流量发给这个 Service，都共享同一个算法。

| `loadBalancer.type` | 含义 | `consistentHash` |
|--------|------|-------------|
| `RoundRobin` | 加权轮询（默认） | 不适用 |
| `LeastConn` | 最少连接 | 不适用 |
| `Ewma` | EWMA 延迟选择 | 不适用 |
| `ConsistentHash` | 一致性哈希 | 必填——选择哈希 key（见下文） |

当 `type: ConsistentHash` 时 `consistentHash` 为必填项；其他 type 下填写会被拒绝（spec 无效）。

### 最简示例：最少连接

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: api-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: api-service
  loadBalancer:
    type: LeastConn
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - backendRefs:
        - name: api-service
          port: 8080
```

## 算法详解

### RoundRobin（默认）

- 加权轮询：backendRef 上较高的 `weight` 将按比例获得更多流量。
- 无锁 O(1) 选择，带后端健康过滤和降级到健康 endpoint 的机制。
- weight 被解读为**比例**——只有后端之间的相对比重有意义，绝对值无关。`[2, 4]` 与 `[1, 2]` 的行为完全相同。非常大的 weight 会被（通过 GCD）约简，若仍然很大则按比例缩小，有效比例精度约为 1/10,000（0.01%）。这仅影响比该精度更细的分流，远低于任何实际的灰度配置。

### ConsistentHash

- 基于 Ketama 的一致性哈希环：只要 endpoint 集合不变，相同的哈希 key 始终映射到同一个 endpoint。
- 当 endpoint 变化时，只有约 1/N 的 key 重新映射。
- 哈希 key 来自由 `consistentHash.hashOn` + `consistentHash.key` 选择的请求属性。
- 如果所选属性**在某个请求中缺失**（例如指定的请求头不存在），则哈希 key 为空，该请求降级为轮询选择。

`consistentHash.hashOn` 选择哈希来源，`consistentHash.key` 指定具体的 header / cookie / 查询参数名：

| `hashOn` 值 | 哈希 key |
|-----------|---------|
| `header` | `key` 指定的请求头的值 |
| `cookie` | `key` 指定的 cookie 的值 |
| `queryParam` | `key` 指定的查询参数的值 |

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: cache-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: cache-service
  loadBalancer:
    type: ConsistentHash
    consistentHash:
      hashOn: header
      key: X-User-Id
```

### LeastConn

- 选择活跃连接数最少的 endpoint。
- 连接计数以 service 为作用域：同一个 endpoint IP 在不同 service 下独立计数。
- 请求连接时计数加一，请求完成时减一；重试请求的计数正确递减，不会造成计数泄漏。
- 新加入的 endpoint（计数为 0）优先被选中；被移除的 endpoint 优雅 drain——不接受新请求，已有请求正常完成。

### Ewma

- 选择指数加权移动平均延迟最低的 endpoint。
- 更新公式：`new = alpha × latency + (1 - alpha) × old`，默认 alpha 为 10%。
- 每次请求完成时折算延迟；重试请求丢弃其采样，不污染前一个 endpoint 的平均值。
- 新 endpoint 初始延迟为 1ms（短暂优先），随后收敛到真实延迟。
- 状态以 service 为作用域。

## 配置场景

### 缓存服务按 cookie 一致性哈希

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: cache-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: cache-service
  loadBalancer:
    type: ConsistentHash
    consistentHash:
      hashOn: cookie
      key: session_id
```

### gRPC 服务使用 EWMA

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: grpc-lb-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: grpc-service
  loadBalancer:
    type: Ewma
```

### 默认算法的加权灰度发布

不需要 `EdgionBackendTrafficPolicy`——加权轮询是默认行为。权重仍然写在路由的 `backendRefs` 上：

```yaml
rules:
  - backendRefs:
      - name: stable-service
        port: 8080
        weight: 90
      - name: canary-service
        port: 8080
        weight: 10
```

## 注意事项

- **作用范围**：策略应用于其 `targetRefs` 命名的每个 Service（须与策略同命名空间）——这是**按目的地**而非按路由的配置。所有把流量发到该 Service 的路由都使用同一个算法。
- **冲突**：当多个 `EdgionBackendTrafficPolicy` 定位到同一 Service 时，**创建时间最早**的胜出（时间相同按名称排序）；落败者状态报告 `Conflicted=True`。策略之间没有按字段合并或取并集——详见
  [冲突处理](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md#conflict-resolution)。
- **健康检查与异常点检测**：所有算法都会自动过滤掉不健康 / 已逐出的 endpoint（在同一策略的 `healthCheck` / `outlierDetection` 字段中配置）。
- **权重**：所有算法都遵从 backendRef 上的 `weight` 字段。

## 相关文档

- [后端配置](./backends/README.md)
- [后端主动健康检查](./backends/health-check.md)
- [熔断器（异常点检测）](./backends/circuit-breaker.md)
- [会话保持](./resilience/session-persistence.md)
- [HTTPRoute 总览](./overview.md)
