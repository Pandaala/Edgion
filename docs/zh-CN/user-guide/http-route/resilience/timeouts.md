# 超时配置

配置请求超时以提高系统弹性。

## 配置

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /api
    timeouts:
      request: 30s
      backendRequest: 10s
    backendRefs:
      - name: api-service
        port: 8080
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| request | duration | | | 整个请求超时（包含重试） |
| backendRequest | duration | | | 单次后端请求超时 |

## 超时类型说明

### request - 请求超时

从收到请求到返回响应的总时间，包括：
- 所有重试尝试
- 等待后端响应

### backendRequest - 后端请求超时

单次后端请求的超时时间，不包括重试。

## 示例

### 示例 1: 基本超时配置

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: timeout-example
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      timeouts:
        request: 60s
        backendRequest: 15s
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2: 长连接场景

```yaml
rules:
  - matches:
      - path:
          type: PathPrefix
          value: /stream
    timeouts:
      request: 3600s  # 1 小时
    backendRefs:
      - name: streaming-service
        port: 8080
```

## 相关文档

- [重试策略](./retry.md)
- [会话保持](./session-persistence.md)
