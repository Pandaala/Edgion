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
    type: BandwidthLimit
    config:
      rate: "1mb"
```

### 场景 2：低带宽限制

```yaml
upstreamResponseBodyFilterPlugins:
    type: BandwidthLimit
    config:
      rate: "50kb"
```

### 场景 3：小数速率

支持小数值，且单位不区分大小写：

```yaml
upstreamResponseBodyFilterPlugins:
    type: BandwidthLimit
    config:
      # 1.5 MB/s = 1572864 字节/s
      rate: "1.5mb"
```

---

## 常见问题

### Q1：支持哪些速率格式？

**A**：纯字节数（`"1024"` 或 `"1024b"`），或带 `kb` / `mb` / `gb` 后缀的数字（`"512kb"`、`"1mb"`、`"1gb"`）。支持小数（`"1.5mb"`），后缀不区分大小写（`"512KB"` 有效）。值为必填项，且必须解析为正数——空值、零值或无法解析的速率会导致校验失败。

### Q2：为什么总带宽超过了配置的速率？

**A**：速率限制**作用于每个响应流**，而非每个连接或全局。在 HTTP/2 多路复用场景下，同一连接上的 N 个并发流各自享有完整的 `rate`，因此聚合连接带宽可达 N × `rate`。跨路由、跨连接、跨 Pod 之间没有共享的令牌桶——如需聚合限速，请使用外部机制（LB QoS、内核 `tc` 或 Service Mesh 策略）。

### Q3：为什么 BandwidthLimit 没有效果？

**A**：必须将其放在 `upstreamResponseBodyFilterPlugins` 阶段。放在 `requestPlugins` 或 `upstreamResponseFilterPlugins` 中无效，因为限速是通过延迟上游响应体 chunk 的发送来实现的。

---

## 注意事项

1. 此插件仅限制下游（客户端方向）的响应带宽，不限制上游请求带宽
2. 必须放在 `upstreamResponseBodyFilterPlugins` 中，放在 `requestPlugins` 中无效
3. **限速作用域：速率作用于每个响应流（每个 HTTP 请求），既不是按连接也不是全局。**
   插件在 chunk 之间不维护状态，对每个上游 body chunk 返回 `chunk_size / rate` 的延迟，
   Pingora 在同一响应内的下一个 chunk 发送前等待该延迟。由此带来的实际表现：
   - 每个正在进行的响应独立享有约 `rate` 字节/秒的吞吐。
   - HTTP/2 多路复用：同一连接上的 N 个并发流各自获得完整 `rate`，整条连接的聚合带宽可达 N × `rate`。
   - HTTP/1.1 keep-alive：同一连接上串行的多次响应各自独立享有完整 `rate`。
   - 跨路由、跨连接、跨 Pod 都没有共享的令牌桶。若需要"总量限速"，应依赖外部机制（LB 层 QoS、内核 `tc`、或 Service Mesh 策略）。

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
      type: BandwidthLimit
      config:
        rate: "500kb"
```

## 相关文档

- [限流（单实例）](./rate-limit-local.md)
- [响应重写](./response-rewrite.md)
