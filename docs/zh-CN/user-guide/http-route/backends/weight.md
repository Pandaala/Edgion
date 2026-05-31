# 权重配置

使用权重实现流量分配，支持灰度发布、蓝绿部署等场景。

## 基本配置

```yaml
backendRefs:
  - name: app-v1
    port: 8080
    weight: 90
  - name: app-v2
    port: 8080
    weight: 10
```

90% 流量到 v1，10% 流量到 v2。

## 配置参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `weight` | Integer | `1` | 此后端在规则内的相对流量份额 |

## 权重的分配方式

权重是**相对值**，不是百分比。每个后端接收 `weight / 所有权重之和` 的规则流量。`90`/`10` 和 `9`/`1` 的流量分配效果相同。

## 默认行为

- 省略 `weight` 时默认为 `1`。多个后端都不指定权重时，流量均等分配。
- `weight: 0` 的后端不接收流量，但保留在配置中——适合停放蓝绿部署中空闲一侧的后端。

## 全有或全无规则

在同一条规则中，**要么每个 `backendRef` 都设置 `weight`，要么都不设置**。在同一规则中混用带权重和不带权重的后端会被拒绝，导致该规则的后端无法解析。

```yaml
# ❌ 无效：一个后端设置了权重，另一个没有
backendRefs:
  - name: app-v1
    port: 8080
    weight: 90
  - name: app-v2
    port: 8080

# ✅ 有效：两个后端都设置了权重
backendRefs:
  - name: app-v1
    port: 8080
    weight: 90
  - name: app-v2
    port: 8080
    weight: 10
```

## 示例

### 示例 1：灰度发布

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: canary-release
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: app-stable
          port: 8080
          weight: 95
        - name: app-canary
          port: 8080
          weight: 5
```

### 示例 2：蓝绿部署

```yaml
# 蓝环境 100%
backendRefs:
  - name: app-blue
    port: 8080
    weight: 100
  - name: app-green
    port: 8080
    weight: 0

# 切换后：绿环境 100%
backendRefs:
  - name: app-blue
    port: 8080
    weight: 0
  - name: app-green
    port: 8080
    weight: 100
```

### 示例 3：A/B 测试

```yaml
rules:
  # 带特定头的请求 → B 版本
  - matches:
      - headers:
          - name: X-Test-Group
            value: B
    backendRefs:
      - name: app-b
        port: 8080
  # 其他请求 → A 版本
  - backendRefs:
      - name: app-a
        port: 8080
```

## 注意事项

- 若规则中所有后端的 `weight` 均为 `0`，则没有可选后端，对该规则的请求返回 5xx。请始终保留至少一个非零权重的后端。
- 权重控制规则流量的分配方式，不影响正在进行中的连接重试到哪个后端。无论权重如何，已被健康检查或熔断器标记为不健康的端点仍会被跳过。

## 相关文档

- [Service 引用](./service-ref.md)
- [后端 TLS](./backend-tls.md)
- [后端主动健康检查](./health-check.md)
- [熔断器](./circuit-breaker.md)
