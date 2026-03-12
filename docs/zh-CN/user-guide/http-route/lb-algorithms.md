# 负载均衡策略配置指南

> **🔌 Edgion 扩展**
> 
> 通过 `ExtensionRef` 配置负载均衡算法是 Edgion 的扩展功能。

本文档说明如何通过 HTTPRoute 的 `extensionRef` 配置负载均衡算法。

## 概述

Edgion 默认使用 **RoundRobin（加权轮询）** 负载均衡算法。您可以通过以下方式为特定服务启用其他算法：

- **ConsistentHash (Ketama)**: 一致性哈希算法，适用于缓存场景
- **LeastConnection**: 最少连接算法，适用于长连接场景
- **EWMA**: 基于指数加权移动平均的延迟感知算法，适用于后端延迟差异大的场景

## 支持的算法

| 算法名称 | 别名 | 说明 | 适用场景 |
|---------|------|------|----------|
| `ketama` | `consistent-hash`, `consistent` | 一致性哈希（Ketama），相同 key 始终路由到同一后端 | 缓存、会话保持 |
| `leastconn` | `least-connection`, `leastconnection`, `least_connection` | 选择活跃连接数最少的后端 | gRPC streaming、WebSocket、长连接 |
| `ewma` | - | 选择 EWMA 延迟最低的后端 | 后端延迟差异大、混合机型 |

不配置时默认使用 **RoundRobin**，支持权重（`weight`）。

## 配置方式

在 HTTPRoute 的 filter 中通过 `extensionRef.name` 指定负载均衡算法。算法配置会自动应用到该 rule 的所有 backendRefs。

### 基本示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: my-route
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - api.example.com
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            name: ketama
      backendRefs:
        - name: my-service
          port: 8080
```

## 算法详解

### RoundRobin（默认）

- 加权轮询：`weight` 越大，被选中的概率越高
- 单个原子计数器递增，无锁选择
- 支持 backend 健康过滤和 fallback

### ConsistentHash

- 基于 Ketama 算法的一致性哈希环
- 相同的 hash key 在 backend 不变时始终映射到相同的 backend
- 当 backend 变化时，只有约 1/N 的 key 重新映射
- hash key 可从 Header / Cookie / Query / 源 IP 中提取
- 当无法提取 hash key 时，自动降级为 RoundRobin

**ConsistentHash 的 hashOn 配置**：

```yaml
extensionRef:
  name: ketama:header:X-User-Id    # 按 Header 哈希
  # 或: ketama:cookie:session_id   # 按 Cookie 哈希
  # 或: ketama:query:user_id       # 按 Query 参数哈希
  # 或: ketama:source_ip           # 按源 IP 哈希
  # 或: ketama                     # 默认按源 IP 哈希
```

### LeastConnection

- 选择当前活跃连接数最少的 backend
- service 级别隔离：同一个 IP 在不同 service 下独立计数
- 请求开始时 +1，请求完成时 -1
- 新 backend 因连接数为 0 被优先选中
- 被移除的 backend 优雅 drain：不接受新请求，等待已有请求完成

### EWMA

- 选择 EWMA 延迟最低的 backend
- EWMA 公式：`new = alpha × latency + (1 - alpha) × old`，默认 alpha = 10%
- 每次请求完成后更新延迟值
- 新 backend 默认延迟 1ms，会被短暂优先选中，随后收敛到真实延迟
- service 级别隔离

## 工作原理

1. **策略提取**：当 HTTPRoute 被创建或更新时，Edgion 扫描所有 `ExtensionRef` 类型的 filter
2. **算法解析**：从 `extensionRef.name` 中解析算法名称
3. **服务映射**：将算法应用到该 rule 的所有 backendRefs 指定的服务
4. **策略存储**：将服务到算法的映射存储到全局 PolicyStore 中
5. **按需加载**：请求到达时，根据服务的策略选择对应的负载均衡算法

## 生命周期管理

- **引用计数**：PolicyStore 跟踪每个服务被多少个 HTTPRoute 引用
- **自动清理**：当最后一个引用该服务的 HTTPRoute 被删除时，对应的策略自动清理
- **缓存管理**：backend 列表变化时自动清除对应的 LB 缓存（RR 选择器、CH 哈希环），下次请求自动重建
- **运行时状态清理**：Service 删除时自动清理所有运行时状态（连接计数、EWMA 值等）

## 示例场景

### 场景 1: 缓存服务使用一致性哈希

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: cache-route
  namespace: default
spec:
  parentRefs:
    - name: gateway
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            name: ketama:header:X-Cache-Key
      backendRefs:
        - name: redis-cache
          port: 6379
```

### 场景 2: API 服务使用最少连接

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-routes
  namespace: prod
spec:
  parentRefs:
    - name: api-gateway
  hostnames:
    - api.mycompany.com
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /users
      filters:
        - type: ExtensionRef
          extensionRef:
            name: leastconn
      backendRefs:
        - name: user-api
          port: 8080
```

### 场景 3: gRPC 服务使用 EWMA

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: grpc-route
  namespace: prod
spec:
  parentRefs:
    - name: api-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /grpc
      filters:
        - type: ExtensionRef
          extensionRef:
            name: ewma
      backendRefs:
        - name: grpc-service
          port: 50051
```

## 注意事项

1. **算法格式**：`extensionRef.name` 中的算法名称不区分大小写
2. **单一配置**：每个 rule 只能配置一个负载均衡算法
3. **作用范围**：算法配置会应用到同一个 rule 中所有的 backendRefs
4. **默认行为**：未配置策略的服务使用默认的 RoundRobin 算法
5. **Backend 权重**：所有算法都支持 `weight` 配置
6. **健康检查集成**：所有算法都自动集成健康检查过滤

## 故障排查

查看日志中的相关信息：

```bash
# 查看策略提取日志
kubectl logs <edgion-pod> | grep "LB policy"

# 查看策略应用日志
kubectl logs <edgion-pod> | grep "Added LB policies"

# 查看策略清理日志
kubectl logs <edgion-pod> | grep "Removed LB policies"

# 查看 backend draining 日志
kubectl logs <edgion-pod> | grep "Backend marked as draining"

# 查看 service 运行时状态清理日志
kubectl logs <edgion-pod> | grep "Removed service runtime state"
```

常见问题：

- **策略未生效**：检查 `extensionRef.name` 中的算法名称是否正确
- **算法名称错误**：使用支持的算法名称或别名（参见上面的算法表格）
- **ConsistentHash 不稳定**：检查 hash key 是否为空（空时会降级为 RR）
