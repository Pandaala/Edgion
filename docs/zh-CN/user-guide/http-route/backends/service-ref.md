# Service 引用

配置后端服务引用。

## 基本配置

```yaml
backendRefs:
  - name: my-service
    port: 8080
```

## 配置参考

| 字段 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| name | string | ✓ | | Service 名称 |
| namespace | string | | 同路由 | Service 命名空间 |
| port | int | ✓ | | 服务端口 |
| kind | string | | Service | 后端类型 |
| weight | int | | 1 | 权重（用于流量分配） |

## 默认行为

### kind 字段

- 未指定 `kind` 时，默认为 `Service`
- 支持的类型：`Service`、`ServiceClusterIp`、`ServiceExternalName`

### namespace 字段

- 未指定 `namespace` 时，使用 HTTPRoute 所在的 namespace
- 跨命名空间引用需要配置 ReferenceGrant

## 跨命名空间引用

引用其他命名空间的 Service 需要 ReferenceGrant：

```yaml
# 路由配置
backendRefs:
  - name: backend-service
    namespace: backend-ns
    port: 8080

---
# ReferenceGrant（在 backend-ns 中创建）
apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-from-default
  namespace: backend-ns
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: default
  to:
    - group: ""
      kind: Service
```

## 相关文档

- [权重配置](./weight.md)
- [后端 TLS](./backend-tls.md)
