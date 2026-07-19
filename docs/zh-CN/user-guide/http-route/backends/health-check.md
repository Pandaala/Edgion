# 后端主动健康检查（Health Check）

> **🔌 Edgion 扩展**
>
> 本功能通过 `EdgionBackendTrafficPolicy` 自定义资源为后端启用主动探测，属于 Edgion 扩展能力。

## 概述

主动健康检查会在网关侧周期性探测后端实例（HTTP/TCP/gRPC），并在负载均衡选择时自动跳过不健康实例。
该功能主要用于后端可达但应用层异常、或非 K8s 场景下的就绪判断补充。

健康检查通过挂载到目标 `Service` 的 `EdgionBackendTrafficPolicy` 资源的 `spec.healthCheck` 字段配置——不再是 Service 注解。同一个 `EdgionBackendTrafficPolicy` 还承载了同一 Service 的[异常点检测（熔断）](./circuit-breaker.md)与[负载均衡算法](../lb-algorithms.md)；三者的完整 schema 参考见 [full schema reference](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md)（英文）。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: my-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: my-backend
  healthCheck:
    active:
      type: http
      path: /healthz
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      expectedStatuses:
        - 200
---
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: production
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

策略必须与目标 Service 位于**同一命名空间**（`spec.targetRefs[].kind: Service`，v1 版本仅支持 core API 组）。

## 配置参数

`spec.healthCheck`：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `active` | Object | 无 | 主动探测配置；不配置时不启用健康检查 |

`spec.healthCheck.active` 子字段：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `type` | Enum | `http` | 探测类型。允许值：`http` / `tcp` / `grpc` |
| `path` | String | `/` | HTTP 探测路径（仅 `http` 有效） |
| `port` | Integer | 使用后端端口 | 探测端口覆盖 |
| `interval` | Duration | `10s` | 探测周期 |
| `timeout` | Duration | `3s` | 单次探测超时 |
| `healthyThreshold` | Integer | `2` | 连续成功多少次恢复健康 |
| `unhealthyThreshold` | Integer | `3` | 连续失败多少次标记不健康 |
| `expectedStatuses` | Array\<Integer\> | `[200]` | HTTP 期望状态码（仅 `http` 有效） |
| `host` | String | 无 | HTTP Host 头覆盖（仅 `http` 有效） |
| `grpcServiceName` | String | `""`（整个服务器） | gRPC Health Check 的 service name（仅 `grpc` 有效） |

## 行为细节

### 挂载粒度为 Service 级

健康检查配置只能挂载到 Service 级别——不再存在 EndpointSlice / Endpoints 层的覆盖级别。若同一 Service 被多个 `EdgionBackendTrafficPolicy` 定位，**创建时间最早**的策略生效（时间相同按名称排序）；落败的策略会在受影响的 ancestor Gateway 上（`status.ancestors[]`）报告 `Accepted=False`，reason 为 `Conflicted`。完整规则及”胜出者被删除后自动提升下一个”的行为见
[冲突处理](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/20-edgion-backend-traffic-policy.md#conflict-resolution)。

### 运行时最小值（保护下限）

即使配置更小值，运行时仍会做下限保护：

- `interval` 最小按 `1s` 执行
- `timeout` 最小按 `100ms` 执行

### 健康状态生效方式

Edgion 不会直接删掉 LB 中的后端，而是在 `select_with()` 选择阶段过滤不健康实例。
因此：

- 健康状态变化会即时影响选路
- 后端列表仍由 `EndpointSlice/Endpoints` 数据源维护

## 场景示例

### 场景 1：K8s 常规 HTTP 服务健康检查（推荐）

配置 HTTP 探测，覆盖该 Service 全部后端。

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: web-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: web-service
  healthCheck:
    active:
      type: http
      path: /healthz
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      expectedStatuses: [200, 204]
```

### 场景 2：TCP 健康检查

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: legacy-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: legacy-backend
  healthCheck:
    active:
      type: tcp
      port: 9000
      interval: 5s
      timeout: 1s
```

### 场景 3：gRPC 后端健康检查

对于实现了标准 [gRPC Health Checking Protocol](https://github.com/grpc/grpc/blob/master/doc/health-checking.md)（`grpc.health.v1.Health`）的 gRPC 后端：

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: grpc-backend-policy
  namespace: production
spec:
  targetRefs:
    - kind: Service
      name: grpc-backend
  healthCheck:
    active:
      type: grpc
      port: 9090
      interval: 10s
      timeout: 3s
      healthyThreshold: 2
      unhealthyThreshold: 3
      grpcServiceName: "my.package.MyService"  # 可选，空字符串=检查整个服务器
```

网关会调用 `Health/Check` RPC，仅当响应状态为 `SERVING` 时标记后端为健康。

## 注意事项

1. `expectedStatuses` 仅对 `http` 生效，`tcp` 和 `grpc` 模式会忽略该字段。对于 `http`，列表必须非空且每个条目都是有效的 HTTP 状态码（100~599）。
2. `path` 仅对 `http` 模式有效，不能为空；空 `path` 会使整个 `EdgionBackendTrafficPolicy` 变为无效（`Accepted=False`）。
3. `healthyThreshold` 和 `unhealthyThreshold` 必须大于等于 `1`。
4. `interval` 和 `timeout` 必须解析为非零的 duration（例如 `10s`、`500ms`）；否则资源会被判定为 `Accepted=False`——按照宽松准入策略，资源仍会同步，网关侧会跳过该无效 spec。
5. 未配置健康检查段的 Service 不会做探测，默认按“健康”参与负载均衡。
6. `grpcServiceName` 仅对 `grpc` 模式有效。空字符串（默认）表示检查整个服务器的健康状态。

## 当前限制

1. **仅支持主动探测**
   - What: 被动健康检查（按请求失败自动降级）由另一套机制承载——见[熔断器（异常点检测）](./circuit-breaker.md)。
   - Workaround: 通过主动探测缩短故障发现时间，并配合异常点检测实现按请求触发的逐出。
   - Tracking: 不适用——这是设计如此。

2. **HTTP 探测仅支持明文 HTTP**
   - What: 当前没有独立 HTTPS 探测配置项
   - Workaround: 使用 TCP 探测或通过内网 HTTP 健康端点
   - Tracking: 规划中

## 故障排除

### 问题 1：策略已配置但似乎没生效

原因：`EdgionBackendTrafficPolicy` spec 校验失败（某 ancestor 的 `Accepted=False/Invalid`）；或另一个策略定位到同一 Service 并赢得了”最早创建者优先”仲裁（`Accepted=False/Conflicted`）；或 `targetRefs` 尚未解析到已存在的 Service（`ResolvedRefs=False`）。
解决方案：执行 `kubectl get edgionbackendtrafficpolicy <name> -n <namespace> -o yaml`，检查 `status.ancestors[].conditions`（每个 ancestor Gateway 一条记录）。

### 问题 2：探测太频繁导致后端压力增大

原因：`interval` 配置过小。
解决方案：提高 `interval`，建议生产环境从 `5s~30s` 起步。

### 问题 3：两个策略定位到同一 Service，只有一个生效

原因：这是预期行为——整段原子生效、最早创建者优先仲裁；同一时刻针对每个关注点，只有最早的 `EdgionBackendTrafficPolicy` 对该 Service 生效。查看落败策略受影响 ancestor 上的 `status.ancestors[].conditions[].type: Accepted, reason: Conflicted`。
解决方案：合并为单一策略，或删除不应再生效的旧策略（下一个最早的策略会自动被提升）。

## 相关文档

- [Service 引用](./service-ref.md)
- [权重配置](./weight.md)
- [后端 TLS](./backend-tls.md)
- [熔断器（异常点检测）](./circuit-breaker.md)
- [负载均衡算法](../lb-algorithms.md)
- [超时配置](../resilience/timeouts.md)
