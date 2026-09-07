# ExtensionRef 嵌套用法移除迁移指南

**版本：** `<version>`

## 背景

`EdgionPlugin::ExtensionRef` 变体已移除，`EdgionPlugins.spec.requestPlugins / upstreamResponseFilterPlugins / upstreamResponseBodyFilterPlugins / upstreamResponsePlugins` 这四个 stage list 里不再支持 `type: ExtensionRef`。HTTPRoute / GRPCRoute 的 filter list 上的 `ExtensionRef`（Gateway API 标准）保留，HTTPBackendRef.filters 的 `ExtensionRef` 也保留。

## 例 1：原嵌套用法 → 拆成 aggregate EdgionPlugins

**旧（不再支持）：** A 的 requestPlugins 嵌套引用 B：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  namespace: ns
  name: B
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        allow:
          - name: internal-network
            cidrs: ["10.0.0.0/8"]
    - type: RealIp
      config:
        trustedAddresses: ["10.0.0.0/8"]
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  namespace: ns
  name: A
spec:
  requestPlugins:
    - type: ExtensionRef
      config:
        name: B
    - type: BasicAuth
      config:
        secretRef:
          name: basic-auth-users
```

**新（推荐）：** 直接把 B 的 requestPlugins 展开到 A：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  namespace: ns
  name: A
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        allow:
          - name: internal-network
            cidrs: ["10.0.0.0/8"]
    - type: RealIp
      config:
        trustedAddresses: ["10.0.0.0/8"]
    - type: BasicAuth
      config:
        secretRef:
          name: basic-auth-users
```

## 例 2：跨多个 HTTPRoute 复用一段插件链

**旧（嵌套，不再支持）：** A 引用 B，多个 Route 引用 A，从而共享 B 的逻辑：

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata: { namespace: ns, name: A }
spec:
  requestPlugins:
    - type: ExtensionRef
      config:
        name: B
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata: { namespace: ns, name: route-1 }
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            kind: EdgionPlugins
            name: A
```

**新（推荐）：** 多个 Route 直接引用同一份 EdgionPlugins，或在 HTTPRoute filter list 里并列写多个 ExtensionRef：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  namespace: ns
  name: route-1
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            kind: EdgionPlugins
            name: shared-bundle-1
        - type: ExtensionRef
          extensionRef:
            kind: EdgionPlugins
            name: shared-bundle-2
```

## CRD 字段移除

`HTTPRouteFilter.extensionRefMaxDepth` 和 `GRPCRouteFilter.extensionRefMaxDepth` 字段已删除。如果存量 yaml 含有这两个字段，需要清理掉，否则 K8s 会报 unknown field 错误。

## 设计动机

对齐 Envoy filter chain 模型：filter chain 是扁平 list，不是 graph。复用通过"多 Route 引用同一资源"实现（对标 Envoy ECDS），不通过"插件引用插件"。
