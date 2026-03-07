# HTTPRoute 总览

HTTPRoute 是 Gateway API 中用于定义 HTTP 路由规则的核心资源。

## 资源结构

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: default
spec:
  parentRefs:           # 绑定到哪个 Gateway
    - name: my-gateway
      sectionName: http
  hostnames:            # 匹配的域名
    - "example.com"
  rules:                # 路由规则列表
    - matches:          # 匹配条件
        - path:
            type: PathPrefix
            value: /api
      filters:          # 过滤器（可选）
        - type: RequestHeaderModifier
          requestHeaderModifier:
            add:
              - name: X-Custom-Header
                value: "value"
      backendRefs:      # 后端服务
        - name: api-service
          port: 8080
          weight: 100
```

## 核心概念

### parentRefs - 父资源引用

指定此路由绑定到哪个 Gateway 的哪个监听器：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| name | string | ✓ | Gateway 名称 |
| namespace | string | | Gateway 命名空间（默认同路由） |
| sectionName | string | | 监听器名称 |

### hostnames - 主机名

匹配请求的 Host 头：

- 精确匹配：`example.com`
- 通配符匹配：`*.example.com`

### rules - 路由规则

每条规则包含：
- **matches**: 匹配条件（路径、头、查询参数、方法）
- **filters**: 请求/响应处理
- **backendRefs**: 后端服务列表

## 相关文档

- [匹配规则](./matches/README.md)
- [过滤器](./filters/README.md)
- [后端配置](./backends/README.md)
- [弹性配置](./resilience/README.md)
