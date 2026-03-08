# Bandwidth Limit 插件

> **🔌 Edgion 扩展**
> 
> BandwidthLimit 是 `EdgionPlugins` CRD 提供的带宽限制插件，不属于标准 Gateway API。

## 概述

Bandwidth Limit 用于限制下游响应带宽，通过控制 body chunk 的发送速率实现限速。适用于防止大文件下载占满带宽、为不同路由分配不同带宽等场景。

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: bandwidth-limit-plugin
spec:
  upstreamResponseBodyFilterPlugins:
    - enable: true
      type: BandwidthLimit
      config:
        rate: "100kb"
```

> **注意**：此插件在 `upstreamResponseBodyFilterPlugins` 阶段执行，不是在 `requestPlugins` 中。

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `rate` | String | ✅ | 无 | 带宽限制值 |

### rate 格式

| 格式 | 示例 | 说明 |
|------|------|------|
| 纯数字 | `"1024"` | 字节/秒 |
| KB | `"512kb"` | 千字节/秒 |
| MB | `"1mb"` | 兆字节/秒 |
| GB | `"1gb"` | 吉字节/秒 |

---

## 常见配置场景

### 场景 1：限制下载速度

```yaml
upstreamResponseBodyFilterPlugins:
  - enable: true
    type: BandwidthLimit
    config:
      rate: "1mb"
```

### 场景 2：低带宽限制

```yaml
upstreamResponseBodyFilterPlugins:
  - enable: true
    type: BandwidthLimit
    config:
      rate: "50kb"
```

---

## 注意事项

1. 此插件仅限制下游（客户端方向）的响应带宽，不限制上游请求带宽
2. 必须放在 `upstreamResponseBodyFilterPlugins` 中，放在 `requestPlugins` 中无效
3. 限速基于每个连接，非全局限速

---

## 完整示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: download-route
spec:
  parentRefs:
    - name: my-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /download
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: bandwidth-limit-plugin
      backendRefs:
        - name: file-server
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: bandwidth-limit-plugin
spec:
  upstreamResponseBodyFilterPlugins:
    - enable: true
      type: BandwidthLimit
      config:
        rate: "500kb"
```

## 相关文档

- [限流（单实例）](./rate-limit.md)
- [响应重写](./response-rewrite.md)
