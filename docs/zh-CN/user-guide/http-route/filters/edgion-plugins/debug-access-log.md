# DebugAccessLogToHeader 插件

> **🔌 Edgion 扩展**
>  
> DebugAccessLogToHeader 是 `EdgionPlugins` CRD 提供的调试插件，不属于标准 Gateway API。

## 概述

DebugAccessLogToHeader 插件将请求当前的、非终态 access log 快照序列化为 JSON 并注入到响应头中。使用它可以直接通过浏览器网络面板或 `curl -v` 查看路由和上游响应头阶段的信息，无需读取网关日志。

注入的响应头名称为 `X-Debug-Access-Log`。

⚠️ **仅供开发调试使用！请勿在生产环境启用！** Access log 可能包含敏感的路由和上游信息，且会增大每个响应的体积。

## 快速开始

### 最简配置

本插件在上游响应 Filter 阶段运行（需要响应存在后才能附加响应头），因此请将其放在 `upstreamResponseFilterPlugins` 中——而不是 `requestPlugins`。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-plugin
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

---

## 配置参数

本插件无配置参数。通过添加带有空 `config: {}` 的条目来启用它。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| _（无）_ | — | — | 无插件参数；是否输出响应头由网关全局 `access_log_sensitive.mode` 策略决定 |

---

## 工作原理

1. 请求正常代理到上游。
2. 在上游响应 Filter 阶段，插件将此时可用的请求状态序列化为 JSON 字符串。
3. 当全局敏感信息模式允许暴露时，将该 JSON 字符串以 `X-Debug-Access-Log` 响应头添加到响应中。

在客户端检查：

```bash
curl -v https://api.example.com/path 2>&1 | grep -i x-debug-access-log
```

### 快照边界

该响应头是响应头阶段的快照，而不是终态 access log 记录。其顶层 `request_info.status` 始终为 `0`，因为该字段稍后才在终态 `logging()` 阶段完成写入。与之独立，`backend_context.upstreams[].status` 可能已经包含某次上游尝试所观察到的状态码。

该快照始终不包含 `ext` 和 `waf`。响应体及总耗时、最终 AI 或流式处理汇总、后续诊断信息，以及当前或后续响应阶段插件的日志都可能缺失或尚未完成。请使用终态 access log 查看完整的请求结果。

### 敏感信息模式行为

网关级 `access_log_sensitive.mode` 设置决定是否可以输出该响应头：

| 模式 | 响应头行为 |
|------|------------|
| `replace`（默认） | 输出可解析的 JSON，并使用配置的替换字符串替换敏感字段 |
| `off` | 输出包含未经脱敏、未经标记原始值的可解析 JSON；这是显式的调试退出选项 |
| `wrap` | 不输出调试响应头，因为带包装标记的值仍保留原始数据；请求继续处理，`wrap mode:header suppressed` 稍后出现在终态 access log 中 |

---

## 常见配置场景

### 1. 在单条调试路由上启用

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-single
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

### 2. 不删除资源的情况下临时禁用

`enable` 标志（默认 `true`）允许你在保留 manifest 的情况下关闭该条目。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-toggle
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      enable: false
      config: {}
```

### 3. 与其他响应 Filter 插件组合使用

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-combo
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: ResponseHeaderModifier
      config:
        add:
          - name: X-Env
            value: staging
    - type: DebugAccessLogToHeader
      config: {}
```

---

## 常见问题

### Q1：添加了插件但没有出现 `X-Debug-Access-Log` 响应头？

**A**：请确认条目在 `upstreamResponseFilterPlugins` 下。本插件只在上游响应 Filter 阶段运行。若配置正确但网关使用 `access_log_sensitive.mode: wrap`，缺少该响应头属于预期行为；请在终态 access log 中查看 `wrap mode:header suppressed`。

### Q2：在生产环境保持开启是否安全？

**A**：不安全。Access log 可能暴露内部路由和上游详情，额外的响应头也会增大每个响应的体积。请将其限定在 staging 环境或临时调试路由使用。

---

## 完整配置示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: debug-route
  namespace: staging
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - "api.example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: debug-access-log
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: debug-access-log
  namespace: staging
spec:
  upstreamResponseFilterPlugins:
    - type: DebugAccessLogToHeader
      config: {}
```

## 相关链接

- [过滤器目录](../README.md)
- [Mock 插件](./mock.md)
