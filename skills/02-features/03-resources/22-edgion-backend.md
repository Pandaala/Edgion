---
name: edgion-backend-features
description: Canonical EdgionBackend AI schema, typed HTTPRoute contract, status, policy attachment, and example.
---

# EdgionBackend

`EdgionBackend` declares an AI provider consumed by an `AiProxy` HTTPRoute rule. It is not a
replacement for a Kubernetes Service and is not a generic external HTTP backend.

## Example

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackend
metadata:
  name: openai
  namespace: production
spec:
  ai:
    provider: OpenAI
    credentialPool:
      credentials:
        - name: primary
          weight: 1
          secretRef:
            name: openai-api-key
    models:
      - name: gpt-4.1
        aliases: [default]
        public: true
    defaults:
      streaming: true
      maxOutputTokens: 4096
```

`OpenAICompatible` additionally requires `spec.ai.endpoint`. Plain HTTP endpoints require the
explicit `allowInsecureHttp: true` opt-in.

## Main fields

| Field | Required | Description |
|-------|----------|-------------|
| `provider` | Yes | `OpenAI`, `OpenAICompatible`, or `Anthropic`; no Gemini/Bedrock enum |
| `endpoint` | Provider-dependent | HTTPS provider base endpoint |
| `allowInsecureHttp` | No | Allows an explicit HTTP endpoint |
| `credentialPool.credentials` | Yes | Bounded weighted credentials backed by Secrets |
| `credentialPool.redisRef` | When distributed quota is used | LinkSys Redis link |
| `credentialPool.onRedisFailure` | No | `Deny` or `Allow` |
| `models` | Yes | Canonical models, aliases, visibility, and optional pricing |
| `defaults.streaming` | No | Allows client streaming by default; false rejects a streaming request |
| `defaults.maxOutputTokens` | No | Request-first fallback for conversion, credential TPM reservation and cost estimation; not an enforced ceiling |

The HTTPRoute reference uses `group: edgion.io`, `kind: EdgionBackend`, and no port. The route
rule must attach `AiProxy`; candidate order and weights come exclusively from
`HTTPRoute.backendRefs`.

## Current protocol and accounting boundary

Managed AiProxy requires `api: openaiChat`, with exact effective POST `/v1/chat/completions`
and local GET `/v1/models`. Native entries omit conversion and accept OpenAI/OpenAICompatible
candidates. Anthropic accepts native `api: anthropicMessages`, or explicit Chat `conversion: chatToAnthropic`.
Native `openaiResponses` accepts OpenAI only; native Messages accepts Anthropic only. Native APIs forbid conversion and injection. `usageObservation`
defaults off, with no provider usage observer or token/cost publication. `provider` enables
existing bounded observation; tokenQuota, credential TPM and LowestUsage require it.
`injectUsageTracking` defaults false and controls declared request mutation independently.

OpenAI JSON usage observation is incremental and bounded. Successful non-streaming
Chat responses require reported usage for numeric settlement. Missing usage,
invalid/incomplete JSON and observation budget failure produce unknown usage.
Chat SSE may estimate usage from decoded content, reasoning and tool arguments.
Selected unary Chat-to-Anthropic conversion
retains one bounded document and defers final commitment under the
[bounded unary resource/transport contract](../../01-architecture/02-gateway/00-overview.md#ai-implementation-boundary).
SSE conversion remains event-based.

`defaults.maxOutputTokens` does not clamp an explicit request value or inject an OpenAI default.
Business token quota is cumulative and entry-scoped; credential TPM uses a separate reservation.
See [operator quota documentation](../../../docs/en/ai-gateway/quotas-and-cost.md).

## Status

Kubernetes stores these native conditions in
`status.controllers[].status.conditions`, selected only by `controllerName`.
FileSystem/etcd retain native conditions; conf_sync carries complete source
status plus native `spec.currentStatus`. See the shared 32-entry
[observation contract](../../01-architecture/01-controller/10-status-management.md#7-status-persistence).

- `Accepted` reports static schema and semantic validation.
- `ResolvedRefs` reports credential Secret resolution.
- `PartiallyInvalid` reports omitted credentials when a usable subset remains.

Secret rotation is hot-reloadable. The Gateway atomically publishes a new immutable backend
generation and in-flight requests keep their captured generation.


The Gateway checks each carried credential Secret against its actual declared
Secret reference before parsing the API key. Name and explicit namespace must
match; an omitted Backend namespace defaults to `default`, while missing Secret
namespace is not a qualified default Secret. Invalid or unresolved credentials
are omitted from the usable pool; a valid subset remains active, and no usable
credentials deactivates the runtime entry. This adds no configuration fields,
Secret UID requirement, or additional reference authorization policy.
