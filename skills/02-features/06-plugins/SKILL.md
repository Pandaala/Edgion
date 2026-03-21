---
name: plugins-features
description: 插件功能：EdgionPlugins 28 个 HTTP 插件和 EdgionStreamPlugins TCP/TLS 层插件。
---

# 06 插件功能

> Edgion 的插件体系分为 HTTP 层插件（EdgionPlugins）和 TCP/TLS 层插件（EdgionStreamPlugins）。

## 文件清单

| 文件 | 主题 |
|------|------|
| [00-plugin-catalog.md](00-plugin-catalog.md) | HTTP 插件完整目录（28 个）与 EdgionPlugins Schema |
| [01-stream-plugins.md](01-stream-plugins.md) | Stream 插件与 EdgionStreamPlugins Schema |

## 插件使用方式

### HTTP 插件绑定

1. **Route 级别** — 通过 `ExtensionRef` Filter 引用 EdgionPlugins：
```yaml
# HTTPRoute / GRPCRoute
rules:
  - filters:
      - type: ExtensionRef
        extensionRef:
          group: edgion.io
          kind: EdgionPlugins
          name: my-plugins
```

2. **全局级别** — 通过 EdgionGatewayConfig.spec.globalPluginsRef：
```yaml
# EdgionGatewayConfig
spec:
  globalPluginsRef:
    - name: global-plugins
      namespace: edgion-system
```

### Stream 插件绑定

通过注解绑定到 Gateway / TCPRoute / TLSRoute：
```yaml
annotations:
  edgion.io/edgion-stream-plugins: "namespace/name"
```
