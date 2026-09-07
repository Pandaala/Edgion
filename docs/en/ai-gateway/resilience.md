# Resilience and Failover

AI resilience combines HTTPRoute retry permission, credential-first rounds inside each
EdgionBackend, route-level backend failover, and optional EdgionBackendTrafficPolicy (EBTP)
destination controls.

## Shared attempt ceiling

`HTTPRoute.retry.attempts` means retries, so total attempts are `1 + attempts`. Edgion supports at
most **seven retries / eight total attempts**. A rule above that ceiling is rejected and dropped
from the data plane; see [AI rule rejection](#ai-rule-rejection) for which status condition
reports it. Route retry, provider failover, and credential rotation share this one ceiling.

## AI rule rejection

AI attachment errors — an over-cap `retry.attempts`, mixed backend kinds, a missing or duplicate
`AiProxy`, or an unsupported backendRef filter — are **per-rule** findings. The offending rule is
always dropped from the data plane; the status condition depends on what is left:

| Situation | Conditions on the affected `parentRef` |
|-----------|----------------------------------------|
| Some rules of the HTTPRoute still serve | `Accepted=True`, `PartiallyInvalid=True/RuleInvalid` with `rules[i]: <reason> (N of M rules active)` |
| No rule of the HTTPRoute serves any more | `Accepted=False/UnsupportedValue` listing every `rules[i]: <reason>`; `PartiallyInvalid` is never `True` (an earlier `True` is cleared to `False`) |

`Accepted=False` removes the whole route from the gateway route table for that parent, so it is
reserved for the case where nothing would serve anyway. A route that mixes a bad AI rule with
valid non-AI rules keeps serving those rules.

**Delegation exception.** The escalation only considers the route's own `spec.rules`. An AI
violation inside a *delegated child* rule is dropped during flattening and always reported as
`PartiallyInvalid=True` — it never sets `Accepted=False`, matching the general rule that a dropped
delegated rule does not reject its parent. If that drop leaves the parent with no rule at all, the
parent stops serving while `Accepted` stays `True`; read the `PartiallyInvalid` message for the
`resolvedRules[i]: <reason>` entries.

## Credential-first rounds

On a retryable failure:

1. Prefer another eligible credential in the same EdgionBackend.
2. Only when the selected backend has no eligible credential, move to another route backend via
   `backendSelection`.
3. After every currently eligible destination was scanned and retry capacity remains, a new round
   may start if the previous round contained at least one dispatched retryable failure.

Pre-dispatch skips (no eligible credential, model mismatch, saturation) consume neither an
HTTPRoute retry nor a retry-budget counter. Every actual dispatch charges the selected AI backend's
retry-budget denominator; a retryable dispatched failure consumes one HTTPRoute retry and that
source service's retry-budget numerator.

## SSE / response commitment

Once Edgion commits to streaming a response body to the client, it does **not** fail over to another
provider or credential for that request. Mid-stream provider failures surface as stream errors
rather than a silent backend switch.

## HTTPRoute retry gates

Retry admission still requires:

- Matching `HTTPRoute.retry.codes` (evaluated on the provider HTTP status before response
  translation)
- Remaining attempt budget
- Aggregate EBTP `retryConstraint` budget on the failed source service (when configured)
- Existing non-idempotent replay rules (`edgion.io/retry-non-idempotent=true` opt-in when needed)

Connect failures before dispatch remain eligible through the normal connection-retry path.

## EdgionBackendTrafficPolicy for AI

EBTP targets `edgion.io/EdgionBackend` directly (same Namespace). It does not target HTTPRoute and
does not define a backend list.

| Section | On EdgionBackend |
|---------|--------------------|
| `connection` | Supported — dial / connect timeout |
| `retryConstraint` | Supported — aggregate anti-storm budget |
| `circuitBreaker` | Supported — aggregate concurrency for the service |
| `outlierDetection` | Supported — passive ejection with credential isolation |
| `loadBalancer` / `healthCheck` / `upstreamAuthority` | Rejected (fail closed) |

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: ai-provider-resilience
  namespace: production
spec:
  targetRefs:
    - group: edgion.io
      kind: EdgionBackend
      name: primary-provider
  connection:
    connectTimeout: 2s
  retryConstraint:
    budget:
      percent: 20
      interval: 10s
  circuitBreaker:
    maxParallelRequests: 200
  outlierDetection:
    consecutiveErrors: 5
    consecutiveGatewayErrors: 3
    consecutiveLocalOriginFailures: 3
    ejectionTime: 30s
    maxEjectionTime: 5m
```

For AI targets, outlier state is isolated by credential alias so one failing or rate-limited
credential does not eject its siblings. After ejection expires, a half-open lease admits one probe
request; other requests continue to bypass that destination until the probe succeeds.

`retryConstraint: {}` enables defaults. Absence disables the aggregate gate. HTTPRoute retry still
decides whether a request may retry; EBTP only limits aggregate retry volume.

The budget and minimum retry rate use monotonic sliding windows with millisecond
precision. System clock adjustments do not change their lifetime, and traffic
older than the immediately preceding window contributes nothing. Each request
retains its captured policy generation through retries, including after a policy
update. Concurrent decisions retain the soft allowance of at most N-1 additional
admissions for N concurrent deciders.

A decision delayed until its sampled history has been overwritten is denied.
Counters never wrap: either counter in a bucket reaching the internal saturation
sentinel (2^40 - 1) denies retries until that history expires.
Exhausting the 48-bit window identifier also denies retries for that budget
instance. These guards apply to both Service and AI backend budgets.

## Related

- [Routing and Selection](./routing.md)
- [Credentials and Limits](./credentials-and-limits.md)
- [HTTPRoute Retry](../user-guide/http-route/resilience/retry.md)
- [Circuit Breaker](../user-guide/http-route/backends/circuit-breaker.md)
