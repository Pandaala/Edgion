---
name: backends-features-detail
description: 后端服务发现、健康检查配置、负载均衡策略完整参考。
---

# 后端功能

## 服务发现

Edgion 支持三种后端发现模式（通过 `conf_center.endpoint_mode` 配置）：

| 模式 | 说明 | 推荐 |
|------|------|------|
| `EndpointSlice` | K8s EndpointSlice API（默认） | ✅ 推荐 |
| `Endpoints` | 传统 Endpoints API | 兼容旧集群 |
| `Both` | 同时使用两种 | 测试 |

### Service 资源

路由通过 `backendRefs` 引用 Service：

```yaml
backendRefs:
  - name: my-service          # Service 名称
    port: 8080                # Service 端口
    weight: 100               # 流量权重
```

Service 变更（如端口更新）会通过 ServiceRefManager 自动触发依赖路由重新处理。

## 健康检查

通过 Service 注解配置健康检查：

```yaml
apiVersion: v1
kind: Service
metadata:
  name: my-service
  annotations:
    edgion.io/health-check: |
      type: http
      path: /healthz
      interval: 10s
      timeout: 5s
      unhealthyThreshold: 3
      healthyThreshold: 2
      port: 8080
```

### 健康检查 Schema

```yaml
edgion.io/health-check: |
  type: String              # http | tcp
  # HTTP 模式
  path: String              # 健康检查路径（http 必填）
  # 通用
  interval: Duration        # 检查间隔
  timeout: Duration         # 超时
  unhealthyThreshold: u32   # 不健康阈值
  healthyThreshold: u32     # 健康恢复阈值
  port: u16                 # 检查端口（可选，默认使用 Service 端口）
```

## 负载均衡策略

Edgion 支持 5 种负载均衡算法：

| 算法 | 说明 | 适用场景 |
|------|------|---------|
| `RoundRobin` | 轮询（默认） | 通用 |
| `EWMA` | 指数加权移动平均（基于响应时间） | 后端性能不均 |
| `LeastConn` | 最少连接 | 长连接场景 |
| `ConsistentHash` | 一致性哈希 | 需要会话亲和 |
| `Weighted` | 加权轮询 | 按权重分配 |

负载均衡策略通过 backendRefs 的 weight 字段和 EdgionGatewayConfig 配置。

## BackendTLSPolicy

网关到后端的 TLS 配置详见 [../05-tls/02-backend-tls-policy.md](../05-tls/02-backend-tls-policy.md)。
