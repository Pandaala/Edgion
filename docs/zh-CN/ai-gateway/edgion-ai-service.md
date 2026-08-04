# EdgionAIService

> API: `edgion.io/v1` | Scope: Namespaced | Short name: `eais`

`EdgionAIService` 定义**一个**提供商 Endpoint、有界凭据池和提供商模型目录。HTTPRoute 通过
`backendRefs` 直接引用这些资源。

## 参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `provider` | Enum | **必填** | `OpenAI`、`OpenAICompatible` 或 `Anthropic` |
| `endpoint` | URL | 提供商默认值 | `OpenAICompatible` 必填；支持可选 path 前缀 |
| `allowInsecureHttp` | Boolean | `false` | 允许 `http://`；仅用于受控开发网络 |
| `credentialPool.credentials` | Array | **必填** | 该 Endpoint 的 1–16 个凭据 |
| `credentialPool.redisRef` | String | 条件必填 | LinkSys Redis 引用（`namespace/name`）；任一凭据配置 RPM/TPM 时必填 |
| `credentialPool.onRedisFailure` | Enum | `Deny` | `Deny` fail-close；`Allow` 在 Redis 故障时绕过凭据限额 |
| `models` | Array | **必填** | 1–256 个提供商模型条目 |
| `models[].name` | String | **必填** | 提供商模型名，最多 512 UTF-8 字节 |
| `models[].aliases` | Array | `[]` | 最多 32 个客户端逻辑名 |
| `models[].pricing` | Object | 无 | 可选的百万 token 固定小数 USD 价格 |
| `defaults.streaming` | Boolean | `true` | 为 `false` 时拒绝客户端流式 |
| `defaults.maxOutputTokens` | Integer | 无 | 转换、TPM 预留与估算成本的输出上限回退值 |

`OpenAI` 默认 `https://api.openai.com`，`Anthropic` 默认 `https://api.anthropic.com`。
EdgionAIService 没有 timeout 字段 — 使用 HTTPRoute `timeouts` 与 EBTP
`connection.connectTimeout`。

## HTTPRoute 后端契约

```yaml
backendRefs:
  - group: edgion.io
    kind: EdgionAIService
    name: primary-provider
    weight: 80
  - group: edgion.io
    kind: EdgionAIService
    name: fallback-provider
    weight: 20
```

规则：

- `group` 必须恰好为 `edgion.io`，`kind` 恰好为 `EdgionAIService`，且不得带 `port`
- Namespace 默认为 HTTPRoute 所在 Namespace；跨 Namespace 需要 ReferenceGrant
- `weight: 0` 禁用该 ref
- 使用 AiProxy 的 Rule 只能有 EdgionAIService 转发后端
- AI backendRefs 拒绝 URL rewrite、redirect、mirror、CORS、external auth 与 ExtensionRef Filter

## 模型与别名

客户端发送逻辑模型名（例如 `chat`）。每个后端通过 `models[].name` 或 `models[].aliases`
独立映射到自己的提供商模型。逻辑模型在故障切换时对配额与 access log 保持稳定。

同一 Service 内名称与别名唯一。可选 `pricing` 为非负、最多六位小数，且不得超过每百万 token
1,000,000 USD。

## 状态

| Condition | 含义 |
|-----------|------|
| `Accepted=False/Invalid` | Endpoint、池、模型、定价或默认值无效 |
| `ResolvedRefs=False` | 没有可用凭据 |
| `PartiallyInvalid=True/InvalidCredentials` | 可用子集已激活；被省略的别名出现在有界 reason 中 |

Secret 解析是 partial-good：部分凭据可用时继续服务；全部不可用时服务失活，请求 fail-close 返回
503。

## 相关

- [凭据与限额](./credentials-and-limits.md)
- [提供商](./providers.md)
- [快速开始完整示例](./quick-start.md#完整示例)
