# 路由与选择

Rule 级 AiProxy 在 HTTPRoute 的 AI 后端中做选择。Route 拥有候选、声明顺序和权重；AiProxy
只拥有选择算法。

## AiProxy 参数

AiProxy 必须通过 Rule 级 EdgionPlugins ExtensionRef 恰好出现一次。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `backendSelection.type` | Enum | `Weighted` | `Weighted`、`Priority`、`LowestLatency`、`LowestUsage` 或 `CostWeighted` |
| `api` | Enum | 必填 | 当前仅实现 `openaiChat`；Responses/Messages 值在实现前校验失败 |
| `conversion` | Enum | 无 | `chatToAnthropic` 显式选择现有有界转换器；所有正权重或提升的候选均须为 Anthropic |
| `usageObservation` | Enum | `off` | `provider` 启用有界观测；off 不安装响应用量观测器 |
| `injectUsageTracking` | Boolean | `false` | 显式修改 Chat SSE usage 参数；要求 provider 观测或声明转换 |
| `onUnknownModel` | Enum | `reject` | `reject` 拒绝未知模型；`passthrough` 要求 `Weighted` 或 `Priority`，且所有有效正权重候选均为 `OpenAI` 或 `OpenAICompatible` |
| `modelRoutes` | Array | `[]` | 结构化模型/header 匹配、canonical 改写、类型化候选过滤与权重覆盖 |
| `tokenQuota` | Object | 无 | 可选的 Redis 逻辑模型 token 配额；省略时不执行配额 Redis 操作 |

AiProxy 只使用 HTTPRoute 声明的候选和重试许可。模型路由可引用已有的类型化候选槽位并明确覆盖权重。

`injectUsageTracking: true` 设置 Chat `stream_options.include_usage`，保留无关的提供商扩展字段，
并可能让客户端收到额外 usage 事件。false 保留客户端自己的 usage 请求。
`usageObservation: provider` 独立启用观测；tokenQuota、凭据 TPM 和 LowestUsage 要求启用它。
RPM、LowestLatency 和显式 CostWeighted 可关闭观测；CostWeighted 仅采集请求估算。
原生候选必须全为 OpenAI 或 OpenAICompatible，包含 modelRoutes 提升的零权重槽位。

## 模型路由

`modelRoutes` 在模型解析后、调度前执行一次。精确匹配优先于末尾通配符；随后按更长的字面前缀、
更多 header 条件排序。同等具体度的歧义配置会被拒绝。

以下是 AiProxy `config` 内的配置片段，资源结构见[完整示例](./quick-start.md#完整示例)。

```yaml
modelRoutes:
  - match:
      model: chat-*
      headers:
        - name: x-tier
          value: premium
    action:
      rewrite: provider-chat-v2
      candidates:
        - backend:
            group: edgion.io
            kind: EdgionBackend
            namespace: production
            name: primary-provider
            occurrence: 0
          weight: 100
```

选择器标识具体的类型化后端声明，包括重复出现的序号。过滤不会移除已声明但缺失、未授权或无效的
流量份额；重试、故障切换与自适应选择仍由原有调度器控制。明确改写在 quota 准入前改变有效逻辑模型。

允许 GET 并匹配该路径的 Route 上，`GET /v1/models` 返回已排序、去重的可达公开 canonical 名称和别名。
带 header 条件的规则使用目录请求的 header；隐藏或不可用的目标会被省略。

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
fail-close，而不是静默移除该候选。默认值不构成强制上限；客户端显式值优先。两个请求字段同时
存在且不相等时会被拒绝。

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
        api: openaiChat
        usageObservation: provider
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
