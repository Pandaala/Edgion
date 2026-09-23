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
| `provider` | Yes | `OpenAI`, `Anthropic`, `Gemini`, or `OpenAICompatible` |
| `endpoint` | Provider-dependent | HTTPS provider base endpoint |
| `allowInsecureHttp` | No | Allows an explicit HTTP endpoint |
| `credentialPool.credentials` | Yes | Bounded weighted credentials backed by Secrets |
| `credentialPool.redisRef` | When distributed quota is used | LinkSys Redis link |
| `credentialPool.onRedisFailure` | No | `Deny` or `Allow` |
| `models` | Yes | Canonical models, aliases, visibility, and optional pricing |
| `defaults.streaming` | No | Provider streaming default |
| `defaults.maxOutputTokens` | No | Provider output-token default |

The HTTPRoute reference uses `group: edgion.io`, `kind: EdgionBackend`, and no port. The route
rule must attach `AiProxy`; candidate order and weights come exclusively from
`HTTPRoute.backendRefs`.

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
