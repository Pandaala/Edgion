# 路由与选择

Rule 级 AiProxy 在 HTTPRoute 的 AI 后端中做选择。Route 拥有候选、声明顺序和权重；AiProxy
只拥有选择算法。

## AiProxy 参数

AiProxy 必须通过 Rule 级 EdgionPlugins ExtensionRef 恰好出现一次。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `backendSelection.type` | Enum | `Weighted` | `Weighted`、`Priority`、`LowestLatency`、`LowestUsage` 或 `CostWeighted` |
| `injectUsageTracking` | Boolean | `true` | 为流式透传请求提供商 usage，同时保留独立的 access log 核算 |
| `onUnknownModel` | Enum | `reject` | `reject` 拒绝未知模型；`passthrough` 仅可用于采用 `Weighted` 或 `Priority` 的 OpenAI-compatible 候选 |
| `tokenQuota` | Object | 无 | 可选的 Redis 逻辑模型 token 配额；省略时不执行配额 Redis 操作 |

AiProxy 只使用 HTTPRoute 声明的候选和重试许可，不维护另一份后端列表。

## 选择算法

选择只扫描支持该请求逻辑模型、权重为正的有效候选：

| 类型 | 行为 |
|------|------|
| `Weighted` | 直接使用 HTTPRoute 后端权重 |
| `Priority` | 按 HTTPRoute 后端声明顺序执行严格的主备切换 |
| `LowestLatency` | 选择滚动 p50 首 token 延迟最低者；完全相同时用 Route 权重打破平局 |
| `LowestUsage` | 选择近期计费 token 用量除以 Route 权重后最低者 |
| `CostWeighted` | 按 Route 权重和请求估算成本的平方反比选择 |

`CostWeighted` 要求每个原本符合条件的候选都有输入/输出价格与输出 token 上限。该上限取自请求的
`max_completion_tokens` 或旧字段 `max_tokens`，缺失时回退到 `defaults.maxOutputTokens`。缺失值会
fail-close，而不是静默移除该候选。

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

## 分层选择

1. AiProxy 用 `backendSelection` 选择符合条件的 HTTPRoute 后端。
2. Edgion 在该 Service 内按权重选择可用凭据。
3. 被 RPM、TPM 或冷却挡住的凭据会被跳过。
4. 仅当所选 Service 没有可用凭据时，才换到另一个 Route 后端。

重试如何与该层次交互，见 [凭据与限额](./credentials-and-limits.md) 与
[弹性与故障切换](./resilience.md)。
