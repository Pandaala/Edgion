# RequestMirror

RequestMirror 是 Gateway API 的标准 HTTPRoute 过滤器，用于将请求镜像到另一个后端。

在 Edgion 里，这个能力常见有两种入口：

1. 直接作为 HTTPRoute 的标准 `RequestMirror` filter
2. 作为可复用的 `EdgionPlugins` `RequestMirror` 配置

如果你只是按 Gateway API 标准方式配置，先看本页；如果你想把镜像逻辑抽成可复用插件资源，再继续看扩展页。

## 最小示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: request-mirror-route
spec:
  parentRefs:
    - name: public-gateway
      sectionName: http
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: mirror-service
              port: 8080
      backendRefs:
        - name: primary-service
          port: 8080
```

## 在 Edgion 中需要知道的点

- 主请求和镜像请求共享同一套 RequestMirror 运行时实现
- 镜像是异步执行的，不决定主请求是否成功
- 如果你需要复用配置、统一引用或扩展更多运行时参数，可以改用 `EdgionPlugins` 形式

## 继续阅读

- [Edgion 扩展版 Request Mirror 插件](../edgion-plugins/request-mirror.md)
- [过滤器总览](../overview.md)
