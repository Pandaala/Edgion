# AI Gateway Observability

## Access log `genai` ledger

AI requests add a top-level `genai` object to the structured access log. It is omitted for non-AI
requests. See [Access Log](../ops-guide/observability/access-log.md) for the full schema.

The final summary includes `provider`, `model`, `input_tokens`, `output_tokens`, optional cache
token counts and `estimated_cost_usd`, `streaming`, `usage_estimated`, optional `quota_status` and
`ttft_ms`, plus a bounded `attempts` array.

`model` is the effective logical model used for upstream dispatch. It normally matches the client's
declared model name or alias; a quota `Downgrade` action replaces it with the configured fallback.
With unknown-model passthrough enabled it can instead be a client-controlled string of up to 512
UTF-8 bytes. Treat it as a potentially high-cardinality access-log field: do not promote it directly
to a Prometheus label or assume a fixed catalog in downstream facets. Apply downstream bucketing
when a bounded aggregation dimension is required. The optional GenAI counter below deliberately
omits a model label.

| Bound | Value |
|-------|-------|
| Max attempt entries | 8 |
| Serialized attempts section | 8 KiB |
| Truncation marker | `attempts_truncated: true` |

Each attempt may record attempt number, namespaced backend key, provider enum, credential alias,
status, fixed failure class, accepted Retry-After seconds, TTFT, reported or estimated tokens, and
estimated cost.

`credential` always means the non-secret configured alias, never Secret data. Logs never contain
provider URLs, resolved IPs, credentials, prompts, completions, tool arguments, or provider error
bodies.

## Optional Prometheus counter

`edgion_gateway_genai_tokens_total` is disabled by default, process-local, and resets when the
Gateway restarts. When enabled, it aggregates every dispatched attempt with fixed labels only
(`direction`, `provider`, `usage_source`). Use it for rates and trends — not as the durable usage
or billing source.

Enable in Gateway config:

```yaml
metrics:
  genai_tokens:
    enabled: true
```

Details: [Metrics](../ops-guide/observability/metrics.md) ·
[Metrics inventory](../ops-guide/observability/metrics-inventory.md).

## Status troubleshooting

| Signal | Meaning |
|--------|---------|
| HTTPRoute `Accepted=False/UnsupportedValue` | Invalid AI attachment, mixed backend kinds, unsupported backend filters, or too many retries — **and no rule of the route serves any more** (see [AI rule rejection](resilience.md#ai-rule-rejection)) |
| HTTPRoute `PartiallyInvalid=True/RuleInvalid` | The same AI attachment errors, but other rules of the route still serve; the message reports `(N of M rules active)` |
| HTTPRoute `ResolvedRefs=False` | Missing, unauthorized, or unsupported backendRef |
| EdgionBackend `Accepted=False/Invalid` | Invalid endpoint, pool, model, or default |
| EdgionBackend `ResolvedRefs=False` | At least one credential Secret is missing, denied, or invalid; the service stays active if another credential is usable |
| EdgionBackend `PartiallyInvalid=True` | Usable subset is active; check bounded invalid credential aliases |
