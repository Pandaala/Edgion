# GRPCRoute 过滤器

GRPCRoute 支持 HTTPRoute 可用过滤器的一个子集。过滤器按声明顺序依次执行，可以修改元数据、镜像流量或调用 Edgion 插件。

## 支持的过滤器

| 过滤器 | 说明 |
|--------|------|
| `RequestHeaderModifier` | 添加、设置或移除 gRPC 请求元数据（headers） |
| `ResponseHeaderModifier` | 添加、设置或移除响应元数据 |
| `RequestMirror` | 将调用镜像到第二个后端，不影响客户端响应 |
| `ExtensionRef` | 通过 `EdgionPlugins` CRD 调用 Edgion 插件 |

gRPC 不支持 `RequestRedirect` 或 `URLRewrite` 过滤器——它们仅适用于 HTTP。

## 修改元数据

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GRPCRoute
metadata:
  name: user-service-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - method:
            type: Exact
            service: user.UserService
            method: GetUser
      filters:
        - type: RequestHeaderModifier
          requestHeaderModifier:
            set:
              - name: x-internal-caller
                value: gateway
            remove:
              - x-debug
      backendRefs:
        - name: user-service
          port: 9000
```

## 镜像流量

将匹配调用的副本发送到第二个后端（例如用于验证新服务版本），客户端仍从主后端接收响应：

```yaml
spec:
  rules:
    - filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: user-service-shadow
              port: 9000
      backendRefs:
        - name: user-service
          port: 9000
```

## 调用 Edgion 插件

通过 `ExtensionRef` 引用 `EdgionPlugins` 资源，可将网关插件（例如认证或限流）应用于 gRPC 调用：

```yaml
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: grpc-auth
      backendRefs:
        - name: user-service
          port: 9000
```

可用插件详见 [Edgion 插件目录](../../http-route/filters/edgion-plugins/README.md)；注意依赖请求 body 的插件对流式 RPC 的行为可能有所不同。

从 `backendRefs[].filters` 引用的 `EdgionPlugins` 不能包含已启用的
`DirectEndpoint`、`DynamicInternalUpstream`、`DynamicExternalUpstream`、`RegionRoute` 或 `Canary`
请求插件。BackendRef 过滤器在选定后端后执行；Controller 会将相应规则标为无效，
Gateway 运行时也会拒绝该引用。

## 相关文档

- [GRPCRoute 总览](../overview.md)
- [GRPCRoute 匹配规则](../matches/overview.md)
- [GRPCRoute 后端配置](../backends/overview.md)
