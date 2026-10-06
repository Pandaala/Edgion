# AI Proxy 插件

> **🔌 Edgion 扩展**
>
> AI Proxy 是 Edgion AI Gateway 的 Rule 级插件入口。完整用户文档已迁至独立的
> [AI Gateway](../../../../ai-gateway/README.md) 专栏。

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

## 用量观测

当前 OpenAI/OpenAICompatible Chat JSON 用量使用有界状态增量提取，原始分片立即转发。编码不支持或 JSON 非法/不完整时，
用量为 unknown，计数和成本省略。执行与结算规则见[配额与成本](../../../../ai-gateway/quotas-and-cost.md)。
`injectUsageTracking` 默认 true，控制请求修改，不控制响应观测器是否安装；当前没有公开的观测关闭配置。
Anthropic JSON 转换复用转换器用量并在提交前完成有界转换，不能概括为立即转发原始分片。
原生 Responses/Messages 及 `api`、`conversion`、`usageObservation` 配置尚不支持，见[提供商](../../../../ai-gateway/providers.md)。
