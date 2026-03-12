# TCPRoute 后端配置

TCPRoute 使用 `backendRefs` 指向四层后端服务。

## 示例

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: mysql-route
  namespace: gateway-system
spec:
  parentRefs:
    - name: edge-gw
      sectionName: tcp-mysql
  rules:
    - backendRefs:
        - name: mysql
          port: 3306
```

## 注意事项

1. TCPRoute 不支持 HTTP 层过滤器。
2. 更复杂的四层控制请结合 Stream Plugins。
