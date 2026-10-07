# AI Gateway Quick Start

Create a Secret, one or more `EdgionBackend` resources, a rule-level `AiProxy` plugin, and an
HTTPRoute. Then point an OpenAI SDK at the route hostname.

## Minimal path

1. Store the provider key in a Secret at key `api-key`.
2. Create an `EdgionBackend` that references the Secret and declares at least one model (and
   optional client-facing aliases).
3. Create an `EdgionPlugins` resource with exactly one enabled `AiProxy`.
4. Attach that plugin to an HTTPRoute rule through `ExtensionRef`, and list only
   `edgion.io/EdgionBackend` backends on that rule.
5. Call `/v1/chat/completions` with any OpenAI-compatible client.

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

`api_key` is only a non-secret SDK compatibility placeholder. Edgion removes client provider
authentication headers and installs the selected Secret-backed credential per attempt. Never put
the real provider key in application configuration. This placeholder does not authenticate clients;
configure ordinary authentication plugins when needed. A final usage chunk can have no choices,
so the example checks before reading one.

## Complete example

Replace both Secret values and example provider model names with models available to each provider
account. The `production` Namespace and `main-gateway` must exist, with Gateway HTTPS for
`ai.example.com`. This example uses the Chat entry; native Responses and Messages use the separate configurations below;
`maxOutputTokens` is a fallback, not an enforced ceiling.

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

## Next steps

- [Concepts](./concepts.md) — who owns candidates, credentials, selection, and resilience
- [Routing and Selection](./routing.md) — choose a `backendSelection` algorithm
- [Resilience and Failover](./resilience.md) — retries, SSE commitment, outlier detection

## Explicit Anthropic conversion

Bind a separate AiProxy entry to exclusively Anthropic backendRefs:

```yaml
api: openaiChat
conversion: chatToAnthropic
usageObservation: off
```

This fragment selects the existing bounded JSON/event SSE converter. It keeps client-visible
usage fields while omitting numerical accounting. Use provider observation for token quotas or TPM.

## Native Responses entry

Use the same OpenAI EdgionBackend and Secret with this plugin configuration fragment, and make
HTTPRoute admit POST `/v1/responses`:

```yaml
requestPlugins:
  - type: AiProxy
    config:
      api: openaiResponses
      usageObservation: provider
```

Omit usageObservation for default-off observation. Send an explicit `model` and native `input`;
use `stream: true` for native typed SSE. Do not set conversion or injectUsageTracking.
See [Providers](./providers.md#native-responses-subset) for continuation and operation restrictions.

## Native Messages entry

Attach a separate AiProxy resource to a route whose backend is an Anthropic
EdgionBackend. The existing Secret, backend and HTTPRoute ownership stays the same.

```yaml
requestPlugins:
  - type: AiProxy
    config:
      api: anthropicMessages
      usageObservation: provider
```

Use the Anthropic SDK with the route's base URL and an application credential;
send an explicit model and max_tokens to POST /v1/messages. Omit usageObservation
for opaque native forwarding without numeric accounting. See [Providers](./providers.md#native-messages-subset).
