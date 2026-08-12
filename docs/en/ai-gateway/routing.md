# Routing and Selection

Rule-level AiProxy chooses among HTTPRoute AI backends. The route owns candidates, declaration
order, and weights; AiProxy owns only the selection algorithm.

## AiProxy parameters

AiProxy must appear exactly once in a rule-level EdgionPlugins ExtensionRef.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `backendSelection.type` | Enum | `Weighted` | `Weighted`, `Priority`, `LowestLatency`, `LowestUsage`, or `CostWeighted` |
| `injectUsageTracking` | Boolean | `true` | Requests provider usage for streaming passthrough while retaining independent access-log accounting |
| `onUnknownModel` | Enum | `reject` | `reject` fails unknown models; `passthrough` is limited to OpenAI-compatible candidates with `Weighted` or `Priority` |
| `tokenQuota` | Object | none | Optional Redis-backed logical-model token quota; omission performs no quota Redis work |

AiProxy does not have `aiServiceRef`, a provider list, backend weights, or retry attempts.

## Selection algorithms

Selection always scans only valid positive-weight route candidates that support the request's
logical model:

| Type | Behavior |
|------|----------|
| `Weighted` | Uses HTTPRoute backend weights directly |
| `Priority` | Uses HTTPRoute backend declaration order as strict primary-to-fallback order |
| `LowestLatency` | Chooses the lowest rolling p50 time to first token; route weight breaks exact ties |
| `LowestUsage` | Chooses the lowest recent charged-token usage divided by route weight |
| `CostWeighted` | Chooses by route weight and inverse-square estimated request cost |

`CostWeighted` requires input/output pricing and an output-token bound for every otherwise eligible
candidate. The bound comes from the request — `max_completion_tokens`, or legacy `max_tokens` —
and falls back to `defaults.maxOutputTokens`. Missing values fail closed instead of silently
removing that candidate.

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

## Hierarchical choice

1. AiProxy chooses an eligible HTTPRoute backend with `backendSelection`.
2. Edgion chooses an eligible credential within that service by weight.
3. A credential blocked by RPM, TPM, or cooldown is skipped.
4. Edgion moves to another route backend only when the selected backend has no eligible credential.

See [Credentials and Limits](./credentials-and-limits.md) and [Resilience and Failover](./resilience.md)
for how retries interact with this hierarchy.
