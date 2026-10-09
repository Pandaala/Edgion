# AI Gateway 快速开始

创建 Secret、一个或多个 `EdgionBackend`、Rule 级 `AiProxy` 插件和 HTTPRoute，然后让
OpenAI SDK 指向该 Route hostname。

## 最短路径

1. 把提供商密钥写入 Secret，键名为 `api-key`。
2. 创建 `EdgionBackend`，引用该 Secret，并声明至少一个模型（及可选的客户端别名）。
3. 创建恰好启用一个 `AiProxy` 的 `EdgionPlugins`。
4. 通过 `ExtensionRef` 把该插件挂到 HTTPRoute Rule，且该 Rule 只转发到
   `edgion.io/EdgionBackend`。
5. 用任意兼容 OpenAI 的客户端调用 `/v1/chat/completions`。

```python
from openai import OpenAI

client = OpenAI(
    base_url="https://ai.example.com/v1",
    api_key="sdk-placeholder",
)

completion = client.chat.completions.create(
    model="chat",
    messages=[{"role": "user", "content": "Summarize this request."}],
    stream=True,
)

for chunk in completion:
    if chunk.choices:
        print(chunk.choices[0].delta.content or "", end="", flush=True)
```

`api_key` 只是非敏感的 SDK 兼容占位符。Edgion 会移除客户端提供商认证 header，并在每次尝试时
安装选中的 Secret 凭据。不要把真实提供商密钥放进应用配置。这个占位符不提供客户端认证；
需要鉴权时另行配置普通认证插件。流末尾的 usage 分片可以没有 choices，因此示例先检查它。

## 完整示例

替换两个 Secret 的值及示例 provider model 名称，确保这些模型在对应提供商账户可用。
`production` Namespace 和 `main-gateway` 必须已存在，Gateway 需为 `ai.example.com` 提供 HTTPS。
示例使用 Chat 入口，不启用原生 Responses 或 Messages；`maxOutputTokens` 是回退值，不是强制上限。

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: primary-key
  namespace: production
type: Opaque
stringData:
  api-key: replace-with-primary-provider-key
---
apiVersion: v1
kind: Secret
metadata:
  name: fallback-key
  namespace: production
type: Opaque
stringData:
  api-key: replace-with-fallback-provider-key
---
apiVersion: edgion.io/v1
kind: EdgionBackend
metadata:
  name: primary-provider
  namespace: production
spec:
  ai:
    provider: OpenAI
    credentialPool:
      credentials:
        - name: primary-account
          secretRef:
            name: primary-key
    models:
      - name: provider-chat-v2
        aliases: [chat]
    defaults:
      streaming: true
      maxOutputTokens: 4096
---
apiVersion: edgion.io/v1
kind: EdgionBackend
metadata:
  name: fallback-provider
  namespace: production
spec:
  ai:
    provider: OpenAI
    credentialPool:
      credentials:
        - name: fallback-account
          secretRef:
            name: fallback-key
    models:
      - name: gpt-fallback-example
        aliases: [chat]
    defaults:
      streaming: true
      maxOutputTokens: 4096
---
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
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: ai-chat-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  hostnames:
    - ai.example.com
  rules:
    - name: chat
      matches:
        - path:
            type: PathPrefix
            value: /v1/chat/completions
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: ai-proxy
      backendRefs:
        - group: edgion.io
          kind: EdgionBackend
          name: primary-provider
          weight: 80
        - group: edgion.io
          kind: EdgionBackend
          name: fallback-provider
          weight: 20
      timeouts:
        backendRequest: 30s
        request: 90s
      retry:
        attempts: 2
        backoff: 10ms
        codes: [429, 502, 503, 504]
```

## 下一步

- [概念](./concepts.md) — 谁拥有候选、凭据、选择与弹性
- [路由与选择](./routing.md) — 选择 `backendSelection` 算法
- [弹性与故障切换](./resilience.md) — 重试、SSE 承诺、outlier 检测

## 显式 Anthropic 转换

为全为 Anthropic 的 backendRefs 单独绑定 AiProxy 入口：

```yaml
api: openaiChat
conversion: chatToAnthropic
usageObservation: off
```

该片段选择现有的有界 JSON／事件 SSE 转换器，保留客户端 usage 字段但不发布数值核算。
tokenQuota 或 TPM 场景需改为 provider 观测。

## 原生 Responses 入口

复用同一个 OpenAI EdgionBackend 和 Secret，插件配置如下；HTTPRoute 接受 POST `/v1/responses`：

```yaml
requestPlugins:
  - type: AiProxy
    config:
      api: openaiResponses
      usageObservation: provider
```

省略 usageObservation 时默认关闭观察。客户端提交显式 model 和原生 input，stream=true 返回原生 typed SSE。
不要设置 conversion 或 injectUsageTracking。续接限制见[提供商](./providers.md#原生-responses-的边界)。

## 原生 Messages 入口

将独立 AiProxy 资源挂到使用 Anthropic EdgionBackend 的 HTTPRoute 上。
Secret、后端和路由的职责保持一致。

```yaml
requestPlugins:
  - type: AiProxy
    config:
      api: anthropicMessages
      usageObservation: provider
```

Anthropic SDK 使用该路由的 base URL 和应用凭据，向 POST /v1/messages 发送显式
model 与 max_tokens。省略 usageObservation 时原生透传且不做数字核算。
边界见[提供商](./providers.md#原生-messages-的边界)。
