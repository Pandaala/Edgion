# AI Proxy 插件

> **🔌 Edgion 扩展**
>
> AI Proxy 是 Edgion AI Gateway 的 Rule 级插件入口。完整用户文档已迁至独立的
> [AI Gateway](../../../../ai-gateway/) 专栏。

## 概览

通过 Rule 级 `EdgionPlugins` ExtensionRef 恰好挂载一个启用的 `AiProxy`。该 Rule 的
HTTPRoute `backendRefs` 只能指向 `edgion.io/EdgionBackend`。AiProxy 负责请求转换、
`backendSelection`、可选 token 配额以及用量/成本核算 — 不负责提供商列表、权重或重试次数。

## 下一步

| 主题 | 文档 |
|------|------|
| 端到端配置 | [AI Gateway 快速开始](../../../../ai-gateway/quick-start.md) |
| 职责模型 | [概念](../../../../ai-gateway/concepts.md) |
| 选择算法 | [路由与选择](../../../../ai-gateway/routing.md) |
| 重试与故障切换 | [弹性与故障切换](../../../../ai-gateway/resilience.md) |
| 提供商 | [提供商](../../../../ai-gateway/providers.md) |
| 配额与成本 | [配额与成本](../../../../ai-gateway/quotas-and-cost.md) |
| Access log / 指标 | [可观测性](../../../../ai-gateway/observability.md) |

## 最小插件形态

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ai-proxy
  namespace: production
spec:
  requestPlugins:
    - type: AiProxy
      config:
        backendSelection:
          type: Priority
        injectUsageTracking: true
        onUnknownModel: reject
```

Secret、EdgionBackend 与 HTTPRoute 见
[完整配置示例](../../../../ai-gateway/quick-start.md#完整示例)。
