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

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| weight | int | | 1 | 权重值（0-1000000） |

## 默认行为

- 未指定 `weight` 时，默认值为 `1`
- `weight: 0` 的后端不接收流量（可用于蓝绿部署切换）

## 示例

### 示例 1: 灰度发布

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: canary-release
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - backendRefs:
        - name: app-stable
          port: 8080
          weight: 95
        - name: app-canary
          port: 8080
          weight: 5
```

### 示例 2: 蓝绿部署

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

### 示例 3: A/B 测试

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

## 相关文档

- [Service 引用](./service-ref.md)
- 灰度发布（即将推出）
