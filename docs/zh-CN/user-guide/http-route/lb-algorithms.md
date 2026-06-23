# 负载均衡算法

> **🔌 Edgion extension**
> 
> 选择非默认的负载均衡算法是 Edgion 的扩展功能，通过 `backendRefs[].filters` 上的 `LBPolicy*` ExtensionRef 实现；这不是标准 Gateway API 的一部分。

Edgion 将请求分发到后端的各个 endpoint。默认使用加权轮询；你可以为某个后端选择一致性哈希、最少连接或基于延迟感知的 EWMA 算法。

## 支持的算法

| 算法 | 选择方式 | 适用场景 |
|------|----------|----------|
| RoundRobin（默认） | 跨健康 endpoint 的加权轮询 | 通用 HTTP 流量 |
| ConsistentHash | 将请求属性哈希到 Ketama 环 | 缓存亲和性、按 key 粘性路由 |
| LeastConnection | 选择活跃连接数最少的 endpoint | 长连接（gRPC stream、WebSocket） |
| EWMA | 选择指数加权移动平均延迟最低的 endpoint | 延迟不均匀的异构后端 |

未配置策略时，使用默认的加权轮询选择器。

## 配置方式

负载均衡策略声明为 **backendRef 上的** `ExtensionRef` filter（不是规则上的 filter）。该 filter 使用 `group: edgion.io` 和 `LBPolicy*` 中的某个 `kind`。这些 ExtensionRef 通过 `kind` 识别，在插件处理之前消费，因此不会进入插件流水线。

| `kind` | 含义 | `name` 字段 |
|--------|------|-------------|
| `LBPolicyConsistentHash` | 一致性哈希 | 选择哈希 key（见下文） |
| `LBPolicyLeastConn` | 最少连接 | `default`（值被忽略） |
| `LBPolicyEwma` | EWMA 延迟选择 | `default`（值被忽略） |

`group` 也可以留空（core API group）——这等同于 `edgion.io`。其他 group 或无法识别的 `kind` 会被忽略，后端保持默认的 RoundRobin 算法。

### 最简示例：最少连接

```yaml
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
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyLeastConn
                name: default
```

## 算法详解

### RoundRobin（默认）

- 加权轮询：backendRef 上较高的 `weight` 将按比例获得更多流量。
- 无锁 O(1) 选择，带后端健康过滤和降级到健康 endpoint 的机制。
- weight 被解读为**比例**——只有后端之间的相对比重有意义，绝对值无关。`[2, 4]` 与 `[1, 2]` 的行为完全相同。非常大的 weight 会被（通过 GCD）约简，若仍然很大则按比例缩小，有效比例精度约为 1/10,000（0.01%）。这仅影响比该精度更细的分流，远低于任何实际的灰度配置。

### ConsistentHash

- 基于 Ketama 的一致性哈希环：只要 endpoint 集合不变，相同的哈希 key 始终映射到同一个 endpoint。
- 当 endpoint 变化时，只有约 1/N 的 key 重新映射。
- 哈希 key 来自由 `name` 字段选择的请求属性。
- 如果所选属性**在某个请求中缺失**（例如指定的请求头不存在），则哈希 key 为空，该请求降级为轮询选择。

`name` 字段使用 `<source>.<key>` 格式选择哈希 key：

| `name` 值 | 哈希 key |
|-----------|---------|
| `header.<HeaderName>` | 该请求头的值 |
| `cookie.<CookieName>` | 该 cookie 的值 |
| `arg.<QueryParam>` | 该查询参数的值 |

`<source>` 前缀必须是 `header`、`cookie` 或 `arg`。无法解析为这三种格式之一的 `name` 会导致策略被**忽略**，后端保持默认的 RoundRobin 算法。

```yaml
backendRefs:
  - name: cache-service
    port: 6379
    filters:
      - type: ExtensionRef
        extensionRef:
          group: edgion.io
          kind: LBPolicyConsistentHash
          # 按 X-User-Id 头进行哈希
          name: header.X-User-Id
```

### LeastConnection

- 选择活跃连接数最少的 endpoint。
- 连接计数以 service 为作用域：同一个 endpoint IP 在不同 service 下独立计数。
- 请求连接时计数加一，请求完成时减一；重试请求的计数正确递减，不会造成计数泄漏。
- 新加入的 endpoint（计数为 0）优先被选中；被移除的 endpoint 优雅 drain——不接受新请求，已有请求正常完成。

### EWMA

- 选择指数加权移动平均延迟最低的 endpoint。
- 更新公式：`new = alpha × latency + (1 - alpha) × old`，默认 alpha 为 10%。
- 每次请求完成时折算延迟；重试请求丢弃其采样，不污染前一个 endpoint 的平均值。
- 新 endpoint 初始延迟为 1ms（短暂优先），随后收敛到真实延迟。
- 状态以 service 为作用域。

## 配置场景

### 缓存服务按 cookie 一致性哈希

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cache-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "cache.example.com"
  rules:
    - backendRefs:
        - name: cache-service
          port: 6379
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyConsistentHash
                name: cookie.session_id
```

### gRPC 服务使用 EWMA

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: grpc-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /grpc
      backendRefs:
        - name: grpc-service
          port: 50051
          filters:
            - type: ExtensionRef
              extensionRef:
                group: edgion.io
                kind: LBPolicyEwma
                name: default
```

### 默认算法的加权灰度发布

不需要策略 filter——加权轮询是默认行为：

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

- **作用范围**：策略应用于其所附加的 backendRef 命名的 service。
- **共享 service**：当多条路由对同一 service 附加不同策略时，该 service 会应用这些策略的并集；LB 层在每次请求时选择合适的策略。只有当最后一条引用该 service 的路由被删除时，策略才会被移除。
- **健康检查**：所有算法都会自动过滤掉不健康的 endpoint。
- **权重**：所有算法都遵从 backendRef 上的 `weight` 字段。

## 相关文档

- [后端配置](./backends/README.md)
- [会话保持](./resilience/session-persistence.md)
- [HTTPRoute 总览](./overview.md)
