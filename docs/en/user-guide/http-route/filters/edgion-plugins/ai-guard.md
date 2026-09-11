# AiGuard Plugin

AiGuard inspects the prompt of a canonical OpenAI Chat Completions request
before it reaches the provider. It selects bounded text spans from the request
messages, evaluates local regex/PII rules over them, and optionally consults one
external evaluator. Exactly one outcome wins per request: allow, mask, or
refuse.

AiGuard is a `requestPlugins` entry.

## AiGuard must be ordered before AiProxy

This is a correctness requirement, not a style preference.

`AiProxy` captures the request body and derives the provider request from that
captured copy. A mask applied **after** `AiProxy` has run would therefore never
reach the provider, and the token estimate used for quota would be computed from
the unmasked text.

Three things enforce this:

1. The gateway records the body version AiGuard settled on. If anything mutates
   the request body between AiGuard and AiProxy — for example a `Dsl` or
   `ProxyRewrite` entry ordered between them — the request fails closed rather
   than silently forwarding an unmasked prompt.
2. An AiGuard that runs after AiProxy fails closed with a fixed error.
3. Placing AiGuard after AiProxy in the same `EdgionPlugins` resource is
   reported by the Controller as `Accepted=False` for enabled source entries.
   The Gateway separately checks entries permitted by the current GatewayClass
   `EdgionGatewayConfig.spec.pluginPolicy`. If that effective order is invalid,
   it rejects requests with a generic HTTP 500 before executing the plugins.
   Disabled entries and scope-restricted entries do not participate in Gateway
   ordering validation; request conditions do not exempt permitted entries.
   An invalid update replaces the previous configuration with this rejection,
   even if the update also contains an invalid DSL or WAF program. Correcting
   the order restores normal execution on the next publication. An existing
   PluginPolicy block keeps its configured rejection status ahead of this gate.

Attaching AiGuard at the GatewayClass-global or Gateway scope is also correct,
since those chains run before route-level plugins.

## Requests that are not chat completions

AiGuard only acts on `POST` requests whose path ends with `/chat/completions` —
the same gate `AiProxy` uses. Every other request passes through untouched, with
no rule evaluation and no evaluator call. This applies to valid configurations;
an invalid same-resource AiGuard/AiProxy order rejects all requests through that
resource, including ordinary traffic.

## Example

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

## Rules

Each rule names a matcher, an action, and optionally the roles it applies to.
Rules are evaluated in declared order over the selected spans. A later rule sees
the text an earlier masking rule produced.

| Field | Meaning |
|---|---|
| `name` | Operator-facing identifier. Never sent to a client or written to a log. |
| `match.type` | `regex` (with `pattern`) or `pii` (with `detector`). |
| `action` | `reject` (default), `mask`, or `allow`. |
| `roles` | Restrict to `system` / `user` / `assistant` / `tool` / `developer`. Absent means every span. |
| `replacement` | Required for `mask`; rejected for the other actions. |

A `reject` wins immediately: no replacement text is computed and, if an
evaluator is configured, it is not called.

### Patterns

Patterns compile at configuration ingest, never per request. A pattern that is
invalid or that exceeds the compiled-size limit makes the resource
`Accepted=False` instead of failing in the request path. The regex engine has no
backreferences and no look-around, so matching time is linear in the input.

### Builtin PII detectors

`email`, `ipAddress`, `creditCard`, `usSocialSecurityNumber`, `awsAccessKeyId`.

These are **best-effort heuristics**, not a compliance control. They will both
miss real values and match text that is not sensitive. Treat them as a starting
point and tune the rule set for your traffic.

## What gets inspected

Only text an LLM would actually read:

- `messages[i].content` when it is a string;
- `messages[i].content[j].text` when the part's `type` is `text`.

Each message must have exactly one string `role`; a missing, non-string, or
duplicate role makes the request unguardable. Duplicate message `content` also
makes the request unguardable, while missing or unsupported message content is
skipped. In a content array, non-object elements and parts with a missing,
non-string, or unrecognized `type` are skipped. A duplicate `type` makes the
request unguardable. When the unique type is `text`, a duplicate `text` makes
the request unguardable, while missing or non-string text is skipped. Text
fields of non-text parts are not inspected.

## Masking

A mask rewrites whole JSON string values in place. The rest of the body —
including field order, other messages, and every unrelated value — is preserved
byte-for-byte, and the whole mask is applied as a single atomic update. If the
masked body would exceed the gateway request-body limit, the request is refused
and the original body is left untouched.

Because AiGuard runs before AiProxy, the masked text is what the provider
receives **and** what token/quota estimation is computed from.

## Refusals

`rejectionMode` selects how a refusal looks to the client:

| Mode | Response |
|---|---|
| `error` (default) | HTTP 403 with a fixed OpenAI-compatible error envelope, `type: content_filter_error`, `code: content_policy_violation`. |
| `contentFilter` | HTTP 200 carrying a valid chat completion whose only choice has `finish_reason: "content_filter"`, so agent chains degrade instead of erroring. Streaming requests receive the SSE form. |

Refusals never disclose which rule matched, what text matched, or any evaluator
detail.

## External evaluator

The `evaluator` block is optional and **off by default**. Configuring it is your
explicit consent to send prompt-derived text outside the gateway.

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

**Data residency.** The gateway sends only the selected bounded span text plus
its structural position, and it sends the text *after* local masking, so
locally masked values never leave the box. It never sends the raw body, request
headers, the model or provider identity, the backend address, or credentials —
apart from the outbound authentication headers you configure.

The call is a single `POST` with one absolute deadline covering both queueing
and the exchange. Retries are rejected at admission, because a decision POST may
already have been committed when a timeout fires. Request and response sizes are
capped, redirects are denied, and proxy environment variables are ignored.

The service answers with exactly one decision:

```json
{"version":"v1","action":"pass"}
{"version":"v1","action":"reject","code":"policy_code","reason":"public reason"}
{"version":"v1","action":"replace","replacements":[
  {"messageIndex":0,"part":{"type":"content"},"current":"reviewed text"}
]}
```

Unknown, duplicate, missing, or oversized fields are treated as a dependency
failure. The evaluator's `code` and `reason` are never reflected to the client
or written to a log.

## Failure policy

`failOpen` defaults to `false`. It applies to dependency, bounds, and
unparseable-request failures.

- `failOpen: false` — the request is refused.
- `failOpen: true` — the request proceeds **unguarded and unmasked**.

`failOpen` never softens a policy reject (local or from the evaluator), and
never softens an ordering fault.

## Bounds

| Field | Default | Maximum when explicitly configured |
|---|---|---|
| `maxSpans` | 256 | 512 |
| `maxSpanBytes` | resolved gateway physical request-body cap | 8 MiB |
| `maxTotalSpanBytes` | resolved gateway physical request-body cap | 8 MiB |
| `maxReplacementBytes` | 64 KiB | 1 MiB |
| `evaluator.maxRequestBytes` | 1 MiB | 1 MiB |
| `evaluator.maxConcurrent` | 64 | 1024 |

When `maxSpanBytes` or `maxTotalSpanBytes` is omitted, it inherits the resolved
gateway physical request-body cap and may therefore exceed the explicit 8 MiB
configuration maximum. The default physical request-body cap is 32 MiB.

Rule count is capped at 64 and a pattern at 4096 bytes.

## Observability

AiGuard writes one bounded entry per request to the access log: the outcome and
a masked-span count. Prompt text, matched text, replacement text, rule names,
patterns, evaluator payloads, endpoints, and credentials are never logged.

## Related

- [ai-proxy.md](ai-proxy.md) — AI backend selection, quota, and provider dispatch
- [guardrail.md](guardrail.md) — response-side streaming decisions
