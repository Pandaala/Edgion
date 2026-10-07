# AI Gateway Observability

## Access log `genai` ledger

AI requests add a top-level `genai` object to the structured access log. It is omitted for non-AI
requests. See [Access Log](../ops-guide/observability/access-log.md) for the full schema.

The final summary includes `provider`, `model`, `streaming`, `usage_source`,
`usage_estimated`, optional `quota_status` and `ttft_ms`, plus a bounded `attempts`
array. Token counts and configured estimated cost are present only for known
reported or valid estimated usage. Summary numbers describe the final attempt, not total consumption
across the request; `attempts` records individual attempts. Business quota terminal settlement and
the optional token metric account for all accountable dispatched attempts, including retries. With
unknown attempts, the known portion is not an authoritative request total.

`usage_source` is `reported`, `estimated`, `unknown`, or `notObserved`. Unknown
usage omits token counts and cost; it does not mean zero. `usage_estimated` is
true only for `estimated`, so false alone does not prove provider-reported usage.
Any unknown attempt, or a mixture of observed and unobserved attempts, makes the
request summary unknown. All-unobserved attempts yield `notObserved`. Known
attempt entries remain available even when the request summary is unknown.
`usageObservation: off` is the default and produces `notObserved`, omitting token counts and cost
for native and converted attempts. `provider` enables bounded observation. `injectUsageTracking`
only controls explicit request-side injection. Converter-intrinsic client usage remains available
when observation is off; RPM and successful TTFT still use the attempt lifecycle.

`model` is the effective logical model used for upstream dispatch. It normally matches the client's
declared model name or alias; an explicit `modelRoutes` rewrite or quota `Downgrade` replaces it with the configured target.
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
Gateway restarts. When enabled, it aggregates known reported/estimated dispatched attempts with fixed labels only
(`direction`, `provider`, `usage_source`). Use it for rates and trends — not as the durable usage
or billing source. Unknown/unobserved attempts emit no token sample, including no fabricated zero. Known attempts are still counted once when another attempt is unknown.

Disabling this counter does not disable usage observation or access-log accounting.

Gateway YAML configuration fragment:

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
