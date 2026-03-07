# 后端主动健康检查（Health Check）

> **🔌 Edgion 扩展**
>
> 本功能通过 `edgion.io/health-check` Annotation 为后端启用主动探测，属于 Edgion 扩展能力。

## 概述

主动健康检查会在网关侧周期性探测后端实例（HTTP/TCP），并在负载均衡选择时自动跳过不健康实例。  
该功能主要用于后端可达但应用层异常、或非 K8s 场景下的就绪判断补充。

## 快速开始

最常见用法是在 `Service` 上配置：

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-backend
  namespace: default
  annotations:
    edgion.io/health-check: |
      active:
        type: http
        path: /healthz
        interval: 10s
        timeout: 3s
        healthyThreshold: 2
        unhealthyThreshold: 3
        expectedStatuses:
          - 200
spec:
  ports:
    - port: 8080
      targetPort: 8080
```

## Annotation 参考

| Annotation | 适用资源 | 类型 | 默认值 | 说明 |
|------------|----------|------|--------|------|
| `edgion.io/health-check` | `Service` / `EndpointSlice` / `Endpoints` | YAML 字符串 | 无 | 配置主动健康检查参数 |

示例值：

```yaml
active:
  type: tcp
  port: 6379
  interval: 5s
  timeout: 1s
  healthyThreshold: 2
  unhealthyThreshold: 3
```

## 配置参数

`edgion.io/health-check` 的 YAML 结构：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `active` | object | ❌ | 无 | 主动探测配置；不配置时不启用健康检查 |

`active` 子字段：

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `type` | `http` \| `tcp` | ❌ | `http` | 探测类型 |
| `path` | string | ❌ | `/` | HTTP 探测路径（仅 `http` 有效） |
| `port` | uint16 | ❌ | 使用后端端口 | 探测端口覆盖 |
| `interval` | duration string | ❌ | `10s` | 探测周期 |
| `timeout` | duration string | ❌ | `3s` | 单次探测超时 |
| `healthyThreshold` | uint32 | ❌ | `2` | 连续成功多少次恢复健康 |
| `unhealthyThreshold` | uint32 | ❌ | `3` | 连续失败多少次标记不健康 |
| `expectedStatuses` | `[]uint16` | ❌ | `[200]` | HTTP 期望状态码（仅 `http` 有效） |
| `host` | string | ❌ | 无 | HTTP Host 头覆盖（仅 `http` 有效） |

## 行为细节与优先级

### 资源层级优先级

同一个 `service_key` 的配置优先级为：

1. `EndpointSlice` annotation
2. `Endpoints` annotation
3. `Service` annotation

### EndpointSlice 冲突处理

当同一服务下多个 `EndpointSlice` 都设置了 `edgion.io/health-check`，且配置不一致时：

- `EndpointSlice` 层配置会被禁用（避免不确定行为）
- 继续回退到下一级（`Endpoints` 或 `Service`）

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

在 `Service` 上配置 HTTP 探测，覆盖该服务全部后端。

```yaml
metadata:
  annotations:
    edgion.io/health-check: |
      active:
        type: http
        path: /healthz
        interval: 10s
        timeout: 3s
        healthyThreshold: 2
        unhealthyThreshold: 3
        expectedStatuses: [200, 204]
```

### 场景 2：非 K8s / Endpoint 模式（仅 Endpoints 可配）

```yaml
apiVersion: v1
kind: Endpoints
metadata:
  name: legacy-backend
  namespace: default
  annotations:
    edgion.io/health-check: |
      active:
        type: tcp
        port: 9000
        interval: 5s
        timeout: 1s
```

### 场景 3：EndpointSlice 层覆盖 Service 层

```yaml
# Service 层默认配置
metadata:
  annotations:
    edgion.io/health-check: |
      active:
        type: http
        path: /healthz

---
# 某组 EndpointSlice 覆盖为 TCP 探测
metadata:
  annotations:
    edgion.io/health-check: |
      active:
        type: tcp
        port: 8081
```

## 注意事项

1. `expectedStatuses` 仅对 `http` 生效，`tcp` 模式会忽略该字段。
2. `path` 必须以 `/` 开头，否则配置会被判定为无效并忽略。
3. `healthyThreshold` 和 `unhealthyThreshold` 必须大于等于 `1`。
4. 未配置健康检查的服务不会做探测，默认按“健康”参与负载均衡。

## 当前限制

1. **仅支持主动探测**
   - What: 当前未实现被动健康检查（按请求失败自动降级）
   - Workaround: 通过主动探测缩短故障发现时间
   - Tracking: 规划中

2. **HTTP 探测仅支持明文 HTTP**
   - What: 当前没有独立 HTTPS 探测配置项
   - Workaround: 使用 TCP 探测或通过内网 HTTP 健康端点
   - Tracking: 规划中

## 故障排除

### 问题 1：配置了 annotation 但似乎没生效

原因：annotation YAML 解析失败或字段校验失败时会被忽略。  
解决方案：检查 YAML 缩进、`path`、阈值与 duration 格式。

### 问题 2：探测太频繁导致后端压力增大

原因：`interval` 配置过小。  
解决方案：提高 `interval`，建议生产环境从 `5s~30s` 起步。

### 问题 3：同一服务多个 EndpointSlice 配置后行为不稳定

原因：配置冲突会触发 EndpointSlice 层禁用并回退。  
解决方案：统一各 EndpointSlice 的 HC 配置，或只保留 Service 层配置。

## 相关文档

- [Service 引用](./service-ref.md)
- [权重配置](./weight.md)
- [后端 TLS](./backend-tls.md)
- [超时配置](../resilience/timeouts.md)
