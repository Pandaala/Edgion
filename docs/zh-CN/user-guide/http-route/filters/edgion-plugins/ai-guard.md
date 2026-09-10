# AiGuard 插件

AiGuard 在规范化 OpenAI Chat Completions 请求到达提供商之前检查其提示词。它从
请求消息中选取有界文本片段，在其上运行本地 regex/PII 规则，并可选地调用一个外部
评估服务。每个请求最终只有一个结果生效：放行、掩码或拒绝。

AiGuard 是 `requestPlugins` 条目。

## AiGuard 必须排在 AiProxy 之前

这是正确性要求，不是风格偏好。

`AiProxy` 会捕获请求体，并基于该副本派生出发往提供商的请求。因此在 `AiProxy`
之后施加的掩码**永远不会**到达提供商，而配额所用的 token 估算也会基于未掩码的
文本计算。

有三重机制保障这一点：

1. 网关记录 AiGuard 落定时的请求体版本。若在 AiGuard 与 AiProxy 之间有任何东西
   修改了请求体（例如中间夹了一个 `Dsl` 或 `ProxyRewrite` 条目），请求会 fail-close，
   而不是静默转发未掩码的提示词。
2. 排在 AiProxy 之后运行的 AiGuard 会以固定错误 fail-close。
3. 在同一个 `EdgionPlugins` 资源中把 AiGuard 放在 AiProxy 之后，会在准入阶段以
   `Accepted=False` 上报。网关会在执行该资源的插件之前，以通用 HTTP 500 拒绝请求。
   禁用条目以及被 PluginPolicy 标记为绕过或阻断的条目不参与排序检查；请求条件不能
   豁免条目。错误更新会用拒绝行为替换旧配置，即使更新同时含有无效的 DSL 或 WAF
   程序。修正顺序后，下一次发布恢复正常执行。已有 PluginPolicy 阻断仍优先返回其
   配置的拒绝状态码。

把 AiGuard 挂在 GatewayClass 全局或 Gateway 作用域同样正确，因为这两条链都先于
路由级插件执行。

## 非 chat completions 请求

AiGuard 仅作用于路径以 `/chat/completions` 结尾的 `POST` 请求——与 `AiProxy` 使用
同一道判定。其余请求原样放行，不做规则评估，也不调用评估服务。这适用于有效配置；
同一资源内的 AiGuard/AiProxy 顺序错误会拒绝经过该资源的所有请求，包括普通流量。

## 示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: guarded-prompts
  namespace: default
spec:
  requestPlugins:
    - type: AiGuard
      config:
        rejectionMode: contentFilter
        rules:
          - name: block-credentials
            match:
              type: pii
              detector: awsAccessKeyId
            action: reject
          - name: redact-emails
            match:
              type: pii
              detector: email
            action: mask
            replacement: "[redacted]"
          - name: internal-only
            match:
              type: regex
              pattern: "project-atlas"
            action: reject
            roles: [user]
    - type: AiProxy
      config:
        backendSelection:
          type: Weighted
```

## 规则

每条规则声明一个匹配器、一个动作，以及可选的作用角色。规则按声明顺序在选中的
片段上求值；后面的规则会看到前面掩码规则产出的文本。

| 字段 | 含义 |
|---|---|
| `name` | 面向运维的标识。绝不会发给客户端，也不会写入日志。 |
| `match.type` | `regex`（配 `pattern`）或 `pii`（配 `detector`）。 |
| `action` | `reject`（默认）、`mask` 或 `allow`。 |
| `roles` | 限定为 `system` / `user` / `assistant` / `tool` / `developer`。缺省表示所有片段。 |
| `replacement` | `mask` 必填；其他动作下填写会被拒绝。 |

`reject` 立即生效：不再计算替换文本；若配置了评估服务，也不会调用它。

### 模式

模式在配置准入时编译，绝不在每请求路径上编译。无效或超出编译体积上限的模式会让
资源变为 `Accepted=False`，而不是在请求路径上失败。所用正则引擎不支持反向引用与
环视，匹配耗时对输入长度是线性的。

### 内置 PII 检测器

`email`、`ipAddress`、`creditCard`、`usSocialSecurityNumber`、`awsAccessKeyId`。

它们是**尽力而为的启发式规则**，不是合规控制手段：既会漏检真实敏感值，也会误命中
非敏感文本。请把它们当作起点，并针对自己的流量调优规则集。

## 检查范围

只检查 LLM 真正会读到的文本：

- `messages[i].content` 为字符串时；
- `messages[i].content[j].text`，且该 part 的 `type` 为 `text` 时。

非文本 part（图片、音频、文件）不做检查。若某条消息的 `role` 或 `content` 缺失、
重复或有歧义，则该请求视为不可守护，按下文的失败策略处理。

## 掩码

掩码就地重写整个 JSON 字符串值。请求体其余部分——包括字段顺序、其他消息以及所有
无关值——保持逐字节不变，且整批掩码作为一次原子更新提交。若掩码后的请求体会超出
网关请求体上限，则拒绝该请求，原始请求体保持不变。

由于 AiGuard 先于 AiProxy 运行，掩码后的文本既是提供商实际收到的内容，也是
token/配额估算所依据的内容。

## 拒绝

`rejectionMode` 决定客户端看到的拒绝形态：

| 模式 | 响应 |
|---|---|
| `error`（默认） | HTTP 403，固定的 OpenAI 兼容错误信封，`type: content_filter_error`、`code: content_policy_violation`。 |
| `contentFilter` | HTTP 200，返回一个合法的 chat completion，其唯一 choice 的 `finish_reason` 为 `"content_filter"`，便于 agent 链路优雅降级而非报错。流式请求返回对应的 SSE 形式。 |

拒绝响应绝不透露命中了哪条规则、命中了什么文本，或任何评估服务的细节。

## 外部评估服务

`evaluator` 配置块是可选的，**默认关闭**。配置它即表示你明确同意把提示词派生的
文本发送到网关之外。

```yaml
        evaluator:
          target:
            url: https://policy.example
          timeoutMs: 1000
          maxResponseBytes: 65536
          maxConcurrent: 64
          request:
            path:
              template: /v1/decision
            headers:
              custom:
                - name: Authorization
                  value:
                    template: "Bearer ${secretRef:default/policy:key}"
```

**数据驻留说明。** 网关只发送选中的有界片段文本及其结构位置，且发送的是**本地掩码
之后**的文本，因此本地已掩码的值绝不会离开网关。它不会发送原始请求体、请求头、
模型或提供商标识、后端地址或凭证——除了你自己配置的出站认证头。

该调用是单次 `POST`，用一个绝对超时同时覆盖排队与交互。重试在准入阶段即被拒绝，
因为超时触发时决策 POST 可能已经提交。请求与响应体积均有上限，重定向被拒绝，
代理环境变量被忽略。

服务端只返回一个决策：

```json
{"version":"v1","action":"pass"}
{"version":"v1","action":"reject","code":"policy_code","reason":"public reason"}
{"version":"v1","action":"replace","replacements":[
  {"messageIndex":0,"part":{"type":"content"},"current":"reviewed text"}
]}
```

未知、重复、缺失或超限字段一律视为依赖失败。评估服务返回的 `code` 与 `reason`
绝不会回显给客户端，也不会写入日志。

## 失败策略

`failOpen` 默认为 `false`，适用于依赖失败、边界超限与请求不可解析这几类失败。

- `failOpen: false` —— 拒绝该请求。
- `failOpen: true` —— 请求**在未守护、未掩码**的情况下继续。

`failOpen` 绝不会软化策略性拒绝（无论来自本地规则还是评估服务），也绝不会软化
顺序错误。

## 上界

| 字段 | 默认值 | 上限 |
|---|---|---|
| `maxSpans` | 256 | 512 |
| `maxSpanBytes` | 8 KiB | 64 KiB |
| `maxTotalSpanBytes` | 网关请求体上限 | 8 MiB |
| `maxReplacementBytes` | 64 KiB | 1 MiB |
| `evaluator.maxRequestBytes` | 1 MiB | 1 MiB |
| `evaluator.maxConcurrent` | 64 | 1024 |

规则数量上限 64 条，单条模式上限 4096 字节。

## 可观测性

AiGuard 每个请求向访问日志写入一条有界记录：结果与掩码片段数。提示词文本、命中
文本、替换文本、规则名、模式、评估服务载荷、端点与凭证均不会被记录。

## 相关文档

- [ai-proxy.md](ai-proxy.md) —— AI 后端选择、配额与提供商分发
- 响应侧流式决策见英文文档 [guardrail.md](../../../../../en/user-guide/http-route/filters/edgion-plugins/guardrail.md)
