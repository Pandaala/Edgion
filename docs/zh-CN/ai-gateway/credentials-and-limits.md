# 凭据与限额

凭据 `tokensPerMinute` 要求 AiProxy 声明 `usageObservation: provider`。只有 RPM 的凭据可使用
默认的 off 观测策略；依赖校验涵盖所有声明的凭据，包括未解析成功的项。

每个 `EdgionBackend` 为同一提供商 Endpoint 拥有一个凭据池。凭据由 Secret 承载；状态与
access log 只出现配置别名。

## 凭据参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `credentials[].name` | String | **必填** | 唯一非敏感别名，1–64 个 ASCII 字符 |
| `credentials[].weight` | Integer | `1` | 加权轮转份额，1–1,000,000 |
| `credentials[].secretRef.name` | String | **必填** | 含 UTF-8 提供商凭据的 Secret，键为 `api-key` |
| `credentials[].secretRef.namespace` | String | EdgionBackend Namespace | 跨 Namespace 需要 ReferenceGrant |
| `credentials[].limits.requestsPerMinute` | Integer | 无 | 可选分布式 RPM |
| `credentials[].limits.tokensPerMinute` | Integer | 无 | 可选分布式 TPM |

不要把提供商密钥写进 EdgionBackend、EdgionPlugins、HTTPRoute 或应用配置。

## Redis 要求

任一凭据配置了 RPM 或 TPM 时，必须设置 `credentialPool.redisRef`。`onRedisFailure` 默认为
`Deny`（fail-close）。`Allow` 在 Redis 不可用时显式绕过凭据限额。

## 预留语义

任一凭据配置 TPM 时，必须在 EdgionBackend 中设置 `defaults.maxOutputTokens`，即使客户端始终
提供输出上限也不能省略。配置验证要求存在这个回退值；运行时仍优先使用请求值。

- **RPM** 在派发前立即预留 1 次请求。
- **TPM** 预留估算输入加上请求声明的输出值；请求未声明时回退到 `defaults.maxOutputTokens`。无法解析输出上限时，TPM 限额凭据无效。
- 已知最终用量会冲正预留。
- 提供商明确返回非 2xx 状态（无论被重试还是作为最终响应）时，按提供商上报的用量冲正；未上报则冲正为 0 token，
  即使错误体不是合法 JSON 或被截断也一样。已派发的请求仍计入 RPM。中间层超时（504、524）仍视为不确定。关闭用量观测时仍保留预留。
- 派发前取消会释放预留。
- 由网关自身中途结束的流（超配额，或配额已无法执行），只有用量已上报或可有效估算时才冲正预留。
  网关主动关闭上游只消除了传输歧义，不能证明用量已知；unknown 用量即使到达 EOS 也保留预留。
- 派发后超时、重置或其他歧义结果会保留完整预留直到 Redis 窗口过期。该预留保护限流，**不是**
  声称提供商消耗了全部预留 token。持久核算以 access log 为准。

`defaults.maxOutputTokens` 不是强制输出上限；请求显式值可覆盖它。凭据 RPM/TPM 保护提供商
凭据，业务 `tokenQuota` 则使用独立的累计配额桶，见[配额与成本](./quotas-and-cost.md)。

## 轮转与 429 冷却

提供商 429 会先轮转凭据。有效的 Retry-After 会为该凭据创建有界冷却；不会驱逐兄弟凭据，也不会让
当前请求睡完整个提供商间隔。同一凭据上已经在途的请求即使成功，也不会缩短提供商冷却。

AI 目标的 outlier / half-open 行为按凭据隔离 — 见 [弹性与故障切换](./resilience.md)。

## 非流式转换的资源限额

选中的非流式 Chat-to-Anthropic 转换会先验证并序列化一个有界的提供商 JSON 文档，
然后才提交最终响应头和响应体。以下配置属于 Gateway 进程 YAML，不属于 EdgionBackend
或 AiProxy 条目，以下仅为该进程配置的片段：

```yaml
ai:
  conversion:
    chat_to_anthropic:
      unary:
        max_input_size: 1MiB
        max_output_size: 1MiB
        hold_timeout: 120s
        memory_budget: 256MiB
        max_json_depth: 64
        max_json_nodes: 16384
        max_key_size: 256B
        max_source_events: 4096
        max_metadata_size: 64KiB
```

| 字段 | 默认值 | 有效范围与含义 |
|------|--------|----------------|
| `max_input_size` | `1MiB` | 1 字节至 `16MiB`；提供商 JSON 字节数 |
| `max_output_size` | `1MiB` | 1 字节至 `16MiB`；序列化后的 Chat 响应字节数 |
| `hold_timeout` | `120s` | `1ms` 至 `1h`；从 Hold 激活开始计时的绝对上限，已有更短的绝对截止时间优先 |
| `memory_budget` | `256MiB` | 计算得到的预留量 `R` 至 `16GiB`；整个进程共享的准入池 |
| `max_json_depth` | `64` | 整数 `1..=64` |
| `max_json_nodes` | `16384` | 整数 `1..=16384`；未知字段和被覆盖的重复值同样计数 |
| `max_key_size` | `256B` | 1 字节至 `256B`；解码后的 JSON 键长度 |
| `max_source_events` | `4096` | 整数 `4..=65536`；源端任务数，包括空响应体事件 |
| `max_metadata_size` | `64KiB` | `8KiB` 至 `64KiB`；保留的响应头与 trailer 元数据总量，每次紧凑投影还限制为最多 64 个字段 |

九个字段均需重启 Gateway 才能修改。省略字段采用默认值；任一 AI 转换子树的未知键、
零值、超出范围或算术溢出都会导致启动失败。准入池至少要容纳一个计算得到的预留量；
不足时，启动错误会报告所需字节数。没有专用 CLI 参数或环境变量覆盖。所有路由及重叠的
插件热更新代际共享同一个池。这些字段不启用转换，也不改变普通 HTTP、原生 AI 或 SSE
的流式路径。

默认配置计算出 **25MiB 的单次预留量和十个并发槽位**。安全边界变化时，预留量也随之变化。
它覆盖转换持有的输入、解析器、DOM、临时值、输出，以及有界的保留任务和紧凑元数据。
这**不是进程 RSS 上限**：原始请求与响应头存储、普通传输及 socket/TLS 缓冲区、其他插件、
分配器开销和碎片均不在其中，也不保证物理内存分配成功。交换清理完成且最后一个输出副本
释放后，准入容量才恢复。

容量耗尽或存在有效的不兼容 Hold 所有者时，在提供商派发前返回固定 **503**。
提供商 JSON 无效或截断、成功响应的媒体类型或编码不受支持、转换或资源超限时，在提交
最终响应头和响应体前返回固定 **502**。Hold 绝对上限到期返回固定 **504**，已有更短的
绝对截止时间仍然生效。所选非流式转换在提供商响应头到达前发生的上游失败或超时，同样使用固定 Chat 502/504 信封。
真正的提供商错误状态及插件明确选择的拒绝响应保持原有处理方式。

完整验证且算术检查通过的提供商用量，在之后的序列化或交付失败时仍然有效。无效或不完整
JSON 的用量为 unknown；固定错误响应不会产生输出字符估算。核算复用转换器发布的用量，
并区分提供商状态与最终客户端结果。

## Responses 的凭据预留

凭据 TPM 要求 provider 观察；只用可靠终态 usage 结算实际用量，也包括 failed/incomplete/cancelled 中已消耗的量。
usage 缺失或后续上下游中断时保守保留预留。请求字符估计和 defaults.maxOutputTokens 只是暂时容量依据，
不能完整预测远端历史或 reasoning。续接只能声明一个凭据绑定，未解析的绑定也计入数量。

## 原生 Messages 凭据

Messages 复用既有凭据池和准入。绑定状态的历史要求一个原始 backend slot 和一个
声明的凭据，禁止改模型和重试。保留版本/beta，清除调用者认证及 workspace selector，
暂不支持 profile 归属。Key 应固定归属一个 workspace；跨主体轮转不在绑定保证内。
见[原生 Messages](./providers.md#原生-messages-的边界)。
