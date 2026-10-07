# Routing and Selection

Rule-level AiProxy chooses among HTTPRoute AI backends. The route owns candidates, declaration
order, and weights; AiProxy owns only the selection algorithm.

## AiProxy parameters

AiProxy must appear exactly once in a rule-level EdgionPlugins ExtensionRef.

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `backendSelection.type` | Enum | `Weighted` | `Weighted`, `Priority`, `LowestLatency`, `LowestUsage`, or `CostWeighted` |
| `api` | Enum | required | `openaiChat` or `openaiResponses`; native Messages remains pending |
| `conversion` | Enum | none | `chatToAnthropic` explicitly selects the existing bounded converter; all positive/promoted candidates must be Anthropic |
| `usageObservation` | Enum | `off` | `provider` installs bounded observation; off creates no response usage observer |
| `injectUsageTracking` | Boolean | `false` | Explicit Chat SSE usage mutation; requires provider observation or declared conversion |
| `onUnknownModel` | Enum | `reject` | `reject` fails unknown models; `passthrough` requires `Weighted` or `Priority` and all valid positive-weight candidates to be `OpenAI` or `OpenAICompatible` |
| `modelRoutes` | Array | `[]` | Structured model/header matching, canonical rewrite, and typed candidate filter/weight overrides |
| `tokenQuota` | Object | none | Optional Redis-backed logical-model token quota; omission performs no quota Redis work |

AiProxy uses only the candidates and retry permission declared by HTTPRoute. A model-route action
may identify existing typed route slots and explicitly override their effective weights.

`injectUsageTracking: true` sets Chat `stream_options.include_usage` without rewriting unrelated
provider extensions and may add a client-visible usage event. False preserves a client's explicit
usage request. Observation is independently selected with `usageObservation: provider`; quota,
credential TPM and LowestUsage require it. RPM, LowestLatency and explicit CostWeighted can use
off; CostWeighted collects request estimates only. Native candidates must all be OpenAI or
OpenAICompatible, including zero-weight slots promoted by modelRoutes.

## Model routes

`modelRoutes` runs once after model parsing and before the existing scheduler.
Exact patterns beat trailing wildcards; then longer literal prefixes and more
header predicates are more specific. Equal-specificity ambiguity is rejected.

This is a fragment inside AiProxy `config`; see the [complete example](./quick-start.md#complete-example)
for the resource structure.

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

Selectors identify one typed backend declaration, including duplicate
occurrence. Filters never remove a declared missing, denied, or invalid traffic
share. The existing scheduler still owns retry, failover, and adaptive choice.
An explicit rewrite changes the effective logical model before quota admission.

`GET /v1/models` on a route that admits GET returns sorted, duplicate-free
reachable public canonical names and aliases. Header-specific rules use the
listing request's headers; hidden and unavailable targets are omitted.

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
removing that candidate. The default is not an enforced ceiling; an explicit client value wins.
If both request fields are present with unequal values, the request is rejected.

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

## Hierarchical choice

1. AiProxy chooses an eligible HTTPRoute backend with `backendSelection`.
2. Edgion chooses an eligible credential within that service by weight.
3. A credential blocked by RPM, TPM, or cooldown is skipped.
4. Edgion moves to another route backend only when the selected backend has no eligible credential.

See [Credentials and Limits](./credentials-and-limits.md) and [Resilience and Failover](./resilience.md)
for how retries interact with this hierarchy.
