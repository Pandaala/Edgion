# AI Gateway 快速开始

创建 Secret、一个或多个 `EdgionAIService`、Rule 级 `AiProxy` 插件和 HTTPRoute，然后让
OpenAI SDK 指向该 Route hostname。

## 最短路径

1. 把提供商密钥写入 Secret，键名为 `api-key`。
2. 创建 `EdgionAIService`，引用该 Secret，并声明至少一个模型（及可选的客户端别名）。
3. 创建恰好启用一个 `AiProxy` 的 `EdgionPlugins`。
4. 通过 `ExtensionRef` 把该插件挂到 HTTPRoute Rule，且该 Rule 只转发到
   `edgion.io/EdgionAIService`。
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
    print(chunk.choices[0].delta.content or "", end="", flush=True)
```

`api_key` 只是非敏感的 SDK 兼容占位符。Edgion 会移除客户端提供商认证 header，并在每次尝试时
安装选中的 Secret 凭据。不要把真实提供商密钥放进应用配置。

## 完整示例

替换两个 Secret 的值，并确保 `main-gateway` 已存在。

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
kind: EdgionAIService
metadata:
  name: primary-provider
  namespace: production
spec:
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
kind: EdgionAIService
metadata:
  name: fallback-provider
  namespace: production
spec:
  provider: Anthropic
  credentialPool:
    credentials:
      - name: fallback-account
        secretRef:
          name: fallback-key
  models:
    - name: claude-example
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
          kind: EdgionAIService
          name: primary-provider
          weight: 80
        - group: edgion.io
          kind: EdgionAIService
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
