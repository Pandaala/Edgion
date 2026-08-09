# Design

## Outcome

AI routing is an extension of the existing HTTPRoute pipeline, not a parallel proxy path:

```text
HTTPRoute rule
  backendRefs[]                         sole candidate set and weights
       |
       +-- EdgionAIService              provider endpoint, models, prices, credentials
       |      |
       |      +-- credential pool       same provider, no protocol translation between keys
       |
       +-- EdgionBackendTrafficPolicy   per-destination resilience and retry budget

EdgionPlugins / AiProxy
  canonical OpenAI request, AI picker, translation, quota, usage and cost
```

No `routing`, upstream/provider list, failover list, or retry-attempt field is added to AiProxy.

## Configuration contract

### HTTPRoute owns destinations, weight, timeouts, and retry

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: chat
  namespace: app
spec:
  parentRefs:
    - name: public
  rules:
    - name: ai-chat
      matches:
        - path:
            type: PathPrefix
            value: /v1/chat/completions
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: ai-chat
      backendRefs:
        - group: edgion.io
          kind: EdgionAIService
          name: openai-prod
          weight: 80
        - group: edgion.io
          kind: EdgionAIService
          name: anthropic-prod
          weight: 20
      timeouts:
        backendRequest: 30s
        request: 90s
      retry:
        attempts: 2
        backoff: 10ms
        codes: [429, 502, 503, 504]
```

Rules for an AI backendRef:

- `group` must be exactly `edgion.io` and `kind` exactly `EdgionAIService`.
- `namespace` defaults to the HTTPRoute namespace. Cross-namespace refs require a matching
  ReferenceGrant whose target is `edgion.io/EdgionAIService`.
- `port` must be absent because the referenced resource owns its endpoint and port.
- `weight: 0` disables a ref. Before any AI-specific picker runs, the gateway performs a weighted
  conformance gate over every positive-weight raw backendRef. The gate chooses either the aggregate
  invalid weight or the aggregate valid weight. An invalid result is recorded in request context;
  if forwarding is reached, it returns the Gateway API-required 500 without AI retry or failover.
  A valid result continues into the AI picker. This preserves the exact invalid-traffic proportion
  even for `Priority` and adaptive pickers without bypassing an earlier terminal rule filter.
- A rule that uses AiProxy must contain only EdgionAIService forwarding backends. Mixed Service
  and AI backends fail closed because one canonical request cannot have both ordinary proxy and
  provider-translation semantics.
- AiProxy is unsupported at backendRef level and must run exactly once at rule level. AI backendRefs
  allow only Gateway API `RequestHeaderModifier` and `ResponseHeaderModifier` in v1.
  `RequestHeaderModifier` is attempt-local: every attempt restores a pristine post-rule-filter
  header/URI snapshot, applies that backend's modifier, then removes and installs provider-owned
  authentication, Host, and protocol headers. `ResponseHeaderModifier` runs only on the final
  non-retried response. `URLRewrite`, `RequestRedirect`, `RequestMirror`, `CORS`, `ExternalAuth`, and
  `ExtensionRef` are rejected for AI backendRefs because they conflict with provider-owned URI,
  introduce terminal/duplicate side effects, or can replay arbitrary plugins.

Route `ResolvedRefs` reports supported kind, object existence, and ReferenceGrant authorization.
It does not mirror the target AI service's Secret condition: an existing service with an unresolved
credential remains a resolved reference, while the service itself reports `ResolvedRefs=False` and
the request path fails closed with 503.

### AiProxy owns only AI request semantics

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ai-chat
  namespace: app
spec:
  requestPlugins:
    - type: AiProxy
      config:
        backendSelection:
          type: Weighted
        injectUsageTracking: true
        onUnknownModel: reject
```

AiProxy reads and validates the canonical client request once, resolves the logical model, applies
token quota, selects among the matched rule's compiled backendRefs, translates each attempt, and
normalizes the final response. `backendSelection` contains only the algorithm; AiProxy never stores
backend names, order, or weights.

The conformance gate described above always runs first. The following algorithms select only among
structurally valid, positive-weight refs that support the request's logical model:

| Type | Candidate source | Selection rule |
|------|------------------|----------------|
| `Weighted` | HTTPRoute backendRefs | Existing weighted round-robin; default |
| `Priority` | HTTPRoute backendRefs | Declaration order is strict primary-to-fallback order |
| `LowestLatency` | HTTPRoute backendRefs | Lowest 30-second rolling p50 TTFT; route weight breaks equal-score ties |
| `LowestUsage` | HTTPRoute backendRefs | Lowest 60-second charged-token count divided by route weight |
| `CostWeighted` | HTTPRoute backendRefs | Integer probability proportional to route weight and inverse-square estimated request cost |

Route weights are never repeated in AiProxy. `Weighted` uses them directly; `Priority` uses only
post-delegation declaration order after the conformance gate; `LowestLatency` uses them for exact
ties; `LowestUsage` treats them as capacity shares; and `CostWeighted` multiplies its inverse-price
score by them. An all-zero route has no selectable backend and fails with 500.

`Priority` deliberately makes backendRef declaration order meaningful only when explicitly selected.
It provides strict primary/secondary/tertiary behavior without duplicating backend names. Priority
tiers are not included in this version.

The client model or alias becomes a request-stable `logical_model`. Each candidate independently
maps it to an attempt-local provider model name. AiProxy quota keys and access logs always use the
logical model, so aliases that map to different provider names cannot bypass `identityModel` quota.
A downgrade atomically replaces the logical model and repeats eligibility and quota admission before
any provider attempt.

`onUnknownModel: passthrough` is valid only when every positive-weight valid candidate uses an
OpenAI-compatible passthrough protocol. The unchanged client model string remains the logical and
provider model. Passthrough is allowed only with stateless `Weighted` and `Priority`; it is invalid
with `LowestLatency`, `LowestUsage`, and `CostWeighted`, so client-controlled names cannot create
unbounded telemetry state or bypass pricing. Each service has at most 256 model entries and each
model has at most 32 aliases.

Adaptive picker state has one stable key `(terminal route UID, rule name, typed AI-service target,
logical model)` and is retained while that normalized tuple remains present. A route UID, rule,
target, or model-catalog change drops the affected state through the existing cache-residue path.
One background tick publishes immutable integer snapshots; requests only scan the bounded route
candidate list and never sort or calculate floating point values.

- `LowestLatency` aggregates successful final responses into one-second histogram buckets and
  publishes the p50 TTFT over the latest 30 complete buckets. A candidate with fewer than eight
  samples is bootstrap-eligible. If no candidate is mature, selection is weighted round-robin over
  bootstrap candidates. If mature and bootstrap candidates coexist, every twentieth selection
  explores a bootstrap candidate by weighted round-robin and the other selections use mature data.
  Failures do not become artificial latency samples; cooldown/ejection removes the failed target.
  Equal integer p50 values use weighted round-robin.
- `LowestUsage` publishes saturating token totals over the latest 60 complete one-second buckets.
  Its integer score is `charged_tokens * 1_000_000 / route_weight`; the lowest score wins and exact
  ties use declaration order. All attempts charged to the logical model contribute.
- `CostWeighted` first parses each decimal USD-per-million price into integer
  micro-USD-per-million, then computes estimated micro-USD with checked `u128` arithmetic:
  `ceil(input_tokens * input_price_per_million / 1_000_000) +
  ceil(max_output_tokens * output_price_per_million / 1_000_000)`. The output bound must come from
  the request or the service model entry. Every otherwise eligible candidate must have both prices
  and a bound; one missing value fails the request as invalid effective configuration rather than
  silently removing that candidate. If any cost is zero, only zero-cost candidates participate by
  route weight. Otherwise `min_cost` is the normalization base and the fixed-point selection weight
  is `route_weight * floor(1_000_000 * min_cost^2 / cost^2)`, with a minimum of one. Overflow fails
  closed before dispatch with a fixed internal reason.

### EdgionAIService owns one provider endpoint and its credential pool

```yaml
apiVersion: edgion.io/v1
kind: EdgionAIService
metadata:
  name: openai-prod
  namespace: app
spec:
  provider: OpenAI
  endpoint: https://api.openai.com
  credentialPool:
    redisRef: app/ai-quota-redis
    onRedisFailure: Deny
    credentials:
      - name: account-a
        secretRef:
          name: openai-account-a
        weight: 100
        limits:
          requestsPerMinute: 500
          tokensPerMinute: 200000
      - name: account-b
        secretRef:
          name: openai-account-b
        weight: 50
  models:
    - name: gpt-example
      aliases: [chat]
      pricing:
        inputPerMillionUsd: "1"
        outputPerMillionUsd: "4"
```

The existing top-level single `secretRef` is replaced; the project is pre-release and does not keep
a compatibility alias. Credential names are stable operator identifiers, never secret values.
Each service has at most 16 credentials; aliases are unique ASCII identifiers of at most 64 bytes.
`defaults.timeout` is also removed: per-attempt and total request deadlines already belong to
`HTTPRoute.timeouts`, while connection establishment belongs to BackendTrafficPolicy. Keeping a
third provider-service timeout would create ambiguous precedence. Provider-specific defaults such as
streaming support and maximum output tokens remain on EdgionAIService.

Selection order is hierarchical:

1. Choose an eligible HTTPRoute AI backend using `AiProxy.backendSelection`.
2. Within that EdgionAIService, choose an eligible credential by configured weight.
3. Skip a credential whose distributed RPM/TPM reservation cannot be admitted or whose failure
   cooldown is active.
4. Move to another HTTPRoute backend only after the selected service has no eligible credential.

Credential limits are optional. When no credential limit is configured, the service creates no
Redis task and `redisRef` is unnecessary; when any limit exists, `redisRef` is required. Distributed
counters extend the existing Redis limiter core with one atomic reserve/reconcile Lua operation; no
second Redis client or ad-hoc counter is introduced. Redis failure is explicit and defaults to
`Deny`.

RPM is reserved as one request immediately before dispatch. TPM reserves estimated input plus the
explicit request/service maximum-output bound, so concurrent requests cannot over-admit the declared
window. A credential with TPM configured but no resolvable maximum-output bound is invalid. Each
attempt owns a reservation keyed by `(AI service UID, credential alias, window)`. On a
known outcome it reconciles that reservation to authoritative usage or conservative estimated input
and refunds output only when an authoritative terminal result proves it unused. A dispatched timeout,
reset, cancellation, or other ambiguous outcome retains the complete input-plus-maximum-output TPM
reservation until the Redis window expires; its access-log cost remains a separately marked estimate.
A pre-dispatch cancellation releases the reservation. A process crash has the same bounded,
conservative window-expiry behavior.

The stable attempt target is `(typed AI-service key, credential alias)`. For a provider 429, parse
delta-seconds or an HTTP date later than the Gateway wall clock through the shared time helper.
Invalid, zero, or past values are treated as absent. A valid hint applies
`clamp(hint, 1s, min(policy.maxEjectionTime.unwrap_or(5m), 5m))` to that credential. Without a valid
hint, the credential is excluded only for the current request unless configured outlier thresholds
open its shared state machine. Selection rotates credentials within the same service before crossing
to another service. Other classified passive failures use the target AI service's
BackendTrafficPolicy outlier thresholds but instantiate the shared state machine per credential. The
service-level circuit breaker remains the aggregate concurrency guard.

Secret resolution is partial-good. `Accepted` still covers structural schema validation.
`ResolvedRefs=True` means at least one credential is usable; `PartiallyInvalid=True` with reason
`InvalidCredentials` reports a bounded count and credential aliases/reason enums for omitted entries.
Zero valid credentials sets `ResolvedRefs=False` and deactivates the service. The handler clears all
old Secret dependency edges before registering every credential ref, so delete-create coalescing
cannot retain a stale key. Every resolved Secret field remains serialized for conf_sync, uses admin
redaction, and is excluded from the CRD schema.

### EdgionBackendTrafficPolicy owns per-destination resilience

EBTP remains a direct backend policy. `targetRefs` accepts one homogeneous backend kind per policy
and never targets HTTPRoute or introduces a forwarding destination:

```yaml
apiVersion: edgion.io/v1
kind: EdgionBackendTrafficPolicy
metadata:
  name: openai-resilience
  namespace: app
spec:
  targetRefs:
    - group: edgion.io
      kind: EdgionAIService
      name: openai-prod
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

Allowed target kinds and fields are:

| Homogeneous target kind | Required/valid policy sections |
|-------------------------|--------------------------------|
| core `Service` | Existing EBTP behavior, unchanged |
| `edgion.io/EdgionAIService` | `connection`, `retryConstraint`, `circuitBreaker`, `outlierDetection` |

Policy identity becomes typed: `(group, kind, namespace, name)`. It must never reuse the existing
plain `namespace/name` Service key because a Service and EdgionAIService may share a name. Targets
remain same-namespace, and mixed target kinds are rejected. EdgionAIService policies contain between
one and 16 unique targets in Rust validation, bounding their new dependency edges and sibling
arbitration work in Kubernetes, filesystem, and etcd modes. Service policies retain their existing
unbounded fan-out and idempotent duplicate-reference behavior.

Canonical target parsing is one shared resource-layer function used by Controller and Gateway.
Structural and field-matrix invalidity rejects the whole policy. Object existence is resolved per
canonical target key: an unresolved target is omitted and reported while resolved siblings remain
eligible independently. Dependencies are still registered for every unresolved target so object
creation self-heals. When an effective winner becomes invalid or unresolved for one key, the next
valid resolved sibling for that key is promoted.

For EdgionAIService targets:

- `connection`, `retryConstraint`, `circuitBreaker`, and `outlierDetection` are valid.
- `loadBalancer`, `healthCheck`, and `upstreamAuthority` are rejected, not ignored. The AI service
  has one provider endpoint; cross-service selection belongs to AiProxy, its Host/SNI comes from the
  endpoint, and public provider health probes need an explicit safe/authenticated contract that this
  task does not invent.
- The outlier state uses the typed AI-service identity as the stable endpoint identity, not a DNS
  result. Its credential child key is the credential alias. DNS rotation therefore does not erase
  circuit state, while one rate-limited credential does not eject its siblings.
- The shared ejection state machine is extended with a single half-open probe lease. The same
  Closed/Open/HalfOpen core is reused by Service endpoints and AI targets; AI does not create a
  second circuit implementation.

The canonical typed policy store owns retry-budget, circuit, outlier, and half-open state. AI-service
snapshots contain stable handles to that store; they never copy live state. When Open expires, one
request acquires the half-open lease with compare-and-swap and all others continue to bypass it.
Success closes and resets the state; a classified failure reopens it with bounded backoff;
cancellation or a pre-dispatch failure releases the lease without a verdict. A guard releases the
lease on `Drop`, and cache-residue cleanup removes state only after no compiled target references it.

`HTTPRoute.retry` decides whether a request may retry; `retryConstraint` is only the aggregate
anti-storm gate. They must both pass. No `aiRetry` section is introduced.

Oldest creation timestamp, then policy name, selects one winner independently for each typed target.
A multi-target policy may win some targets and lose others, preserving the current per-target
conflict/status behavior.

## Controller data model and status

Introduce one canonical backend classifier shared by Controller and Gateway:

```text
core/Service                         -> Service
edgion.io/EdgionAIService           -> EdgionAIService
gateway.networking.k8s.io/HTTPRoute -> RouteDelegation
everything else                     -> Unsupported
```

The Controller adds an `ai_service_ref_mgr` using the existing bidirectional reference manager:

```text
HTTPRoute reconcile
  clear old AI refs
  add every effective terminal AI backend ref

EdgionAIService apply/delete
  enqueue referencing HTTPRoutes and EBTPs through each kind's on_event_key
```

HTTPRoute apply/delete, delegation changes, and parentRef changes requeue EBTPs targeting the union
of old and new terminal AI-service keys so per-Gateway policy ancestors cannot remain stale after a
route adds, drops, delegates, or moves an AI backend. The reverse manager stores typed source kinds;
callers filter HTTPRoute and EBTP dependents rather than accidentally requeueing the wrong kind.

Delegation flattening must carry terminal AI backend tuples and preserve the child route namespace
for ReferenceGrant checks. It also carries terminal route UID and rule name for adaptive-picker state
identity and cleanup. It must not register AI refs as Services.

BackendTrafficPolicy arbitration remains oldest-wins, but runs per typed target. Service-specific
stores retain their existing keys and behavior. Independently synchronized HTTPRoute,
EdgionPlugins, EdgionAIService, and policy resources are eventually consistent; no synthetic atomic
generation is claimed. A request captures its matched route `Arc`, one AiServiceStore snapshot, and
stable handles from the canonical typed policy store. Those request-local captures do not change
mid-attempt. Unchanged normalized policy and target identity retain live retry-budget, circuit, and
telemetry state across resync.

The Controller adds typed EBTP target dependencies for EdgionAIService using the AI-service reverse
index. EBTP changes flow to the gateway through their own conf_sync update and requeue only affected
sibling policies for status arbitration; they do not rebuild or requeue HTTPRoutes. Every reconcile
is clear-before-add, and no handler directly invokes another handler.

The gateway policy store remains the sole writer and publishes one ArcSwap effective destination
snapshot keyed by typed backend identity. Adaptive AiProxy state separately uses
`(route UID, rule name, typed backend, logical model)`, so delete-create of the same route name cannot
reuse telemetry. A request captures one destination-policy snapshot for all attempts. AiProxy's
immutable selection config comes from its compiled rule-level plugin runtime; stable retry-budget,
circuit, outlier, half-open, and adaptive telemetry handles survive an equivalent resync and are
removed through cache-residue cleanup.

An AI-forwarding rule must resolve exactly one enabled AiProxy and it must come from a rule-level
EdgionPlugins ExtensionRef. AiProxy is invalid in global attachment, backendRef filters, or more than
one referenced plugin chain. The EdgionPlugins schema-layer validator restricts AiProxy to
`requestPlugins` and at most one per resource; route reconciliation resolves all rule-level plugin
refs and enforces the cross-resource exactly-one rule. HTTPRoute reconcile clears old plugin edges
and registers every rule-level EdgionPlugins ExtensionRef. EdgionPlugins apply/delete requeues only
referencing HTTPRoutes through their serialized workqueue, so missing/create, content changes, and
delete self-heal the route status.
Violations always drop the offending rule from the gateway runtime and carry a bounded reason
covering missing AiProxy, duplicate AiProxy, mixed forwarding kinds, unsupported AI backendRef
filters, or invalid CostWeighted structural attachment. The status condition follows the HTTPRoute
per-rule precedent: while any rule of the route still serves, the reason rides
`PartiallyInvalid=True/RuleInvalid` with `(N of M rules active)` and `Accepted` is untouched; only
when no rule serves any more is the route set `Accepted=False` with `UnsupportedValue` listing every
reason (and `PartiallyInvalid` is then never `True` — an earlier `True` is cleared to `False` — so
the two conditions never contradict each other).
Route-level `Accepted=False` drops the whole route from the data plane, so it must not be triggered
by a per-rule finding while valid sibling rules remain. Request-specific model/price coverage is checked at runtime
because the logical model is not known during reconciliation. Under the project's lenient CRD
admission policy, "rejected" means this
status plus omission from the gateway runtime, not Kubernetes API admission rejection. Gateway
compilation repeats the invariant fail-closed as defense in depth.

## Gateway request and attempt flow

### Request phase

1. HTTPRoute matching publishes the immutable rule and compiled backendRefs and samples the
   conformance gate once into request context.
2. Rule filters keep their established order. A terminal filter such as RequestRedirect remains
   terminal. If execution reaches AiProxy with an invalid gate result, AiProxy performs no body read,
   quota work, or mutation and the forwarding phase later materializes the required 500.
3. Rule-level AiProxy verifies exactly-one attachment and homogeneous AI targets.
4. It reads and validates the client body once and retains immutable canonical OpenAI bytes.
5. It fixes the logical model, resolves candidate-local provider models, performs quota admission,
   and creates a bounded
   `AiAttemptPlan`. It does not set URI, Host, credential, or ExternalJump.
6. It installs one attempt-aware response processor wrapper.

The AiService store supplies all candidates from one ArcSwap snapshot so a single request cannot mix
resource generations during hot reload.

### Each upstream attempt

Pingora re-enters `upstream_peer` for every attempt. If an AI plan exists:

1. Apply the AiProxy plan's `backendSelection` picker to valid, model-compatible route backendRefs;
   absence means `Weighted`.
2. Apply the destination's BackendTrafficPolicy state and aggregate retry budget.
3. Select an eligible credential inside the chosen service.
4. Rebuild the provider request from immutable canonical bytes.
5. Atomically overwrite the existing request `BodyStorage` snapshot.
6. Remove the complete provider-owned header set, then install the selected credential and provider
   headers.
7. Set URI, Host, timeout, and construct an external peer through the shared DNS/SSRF/TLS path.
8. Record a bounded attempt entry and begin ordinary Pingora request-body replay.

The generic first-write-wins upstream-jump contract is not relaxed. AI attempts use a typed backend
branch in `upstream_peer`; they do not repeatedly mutate the plugin jump slot and require no Pingora
fork change.

AI backendRef RequestHeaderModifier runs from the pristine per-attempt request snapshot;
ResponseHeaderModifier runs only for the final non-retried response. All other backendRef filter
types are rejected for AI routes in this version. Rule-level plugins run once before the attempt
loop.

### Retry arbitration

- HTTP status: existing `HTTPRoute.retry.codes`, before any provider response translation.
- A connect failure before dispatch is safe to retry through the existing connection path.
- A transport failure after dispatch is ambiguous for an AI POST. It retains the existing
  `edgion.io/retry-non-idempotent=true` opt-in; without it, the request does not replay even when no
  response byte arrived.
- Backend-request timeout: existing per-try timeout path.
- Aggregate retry admission: the `retryConstraint` handle captured on the failed dispatched
  AIService attempt. Absence means no aggregate gate.
- Minimum delay: existing `HTTPRoute.retry.backoff`.

`HTTPRoute.retry.attempts` means retries, so total attempts are `1 + attempts`. Edgion supports at
most seven retries and eight total attempts. A rule above seven is dropped from the gateway runtime
and reported through the per-rule status escalation described under AiProxy attachment above
(`PartiallyInvalid` while sibling rules serve, `Accepted=False/UnsupportedValue` once none do); the
gateway also rejects it defensively. The global fallback is capped to the same value. One exclusive
AI retry branch consumes this shared ceiling, so route retry, provider failover, and credential
rotation cannot each create their own attempt budget.

Selection uses bounded rounds. Within a round, the AiProxy picker chooses an untried service;
credentials rotate by weight within that service before the service is excluded and the picker runs
over the remaining services. A pre-dispatch skip such as no usable credential, open circuit, missing
model, or saturation consumes neither an HTTPRoute retry, a retry-budget numerator/denominator, nor
an attempt-ledger slot. Every actual dispatch increments that selected AIService's retry-budget
denominator. Its retryable failure consumes one HTTPRoute retry and the failed source service's
retry-budget numerator before another destination is selected. After every currently eligible
service has been tried, a new round clears only the request-local tried sets when the completed round
contained at least one dispatched retryable failure and retry capacity remains. A full scan with no
dispatch-capable target returns the fixed 503 immediately. Persistent Retry-After, quota, and
ejection exclusions stay active across rounds. This lets Priority fail over, prevents empty-round
loops, and still lets a single-backend route perform configured retries.

On a retryable 429, a valid `Retry-After` or supported rate-limit reset header is captured as a
bounded credential cooldown. The current request rotates credentials, then services, after the
configured route backoff; it never sleeps for the provider's complete Retry-After interval.

Provider error codes embedded in response bodies are not retry triggers. Supporting them would
require response aggregation or speculative buffering and violates the streaming response boundary.

Before advancing, the AI branch checks Pingora's downstream-commit semantics: 100 and 103 do not
commit the final response; a retryable final upstream status may be intercepted before commitment;
101 and any response selected for downstream commitment prohibit failover. Once any streamed final
response is committed, an upstream disconnect is returned to the client and no fallback is attempted.

### Response processing

AiProxy installs one attempt-aware wrapper during the request phase. Retryable response headers are
discarded before the wrapper sees them. On the first final non-retried response header, the wrapper
locks to the selected provider and creates either the Anthropic translator or OpenAI usage observer.
Every later chunk performs one enum match with no store lookup, blocking lock, tracing, or per-chunk
PluginLog allocation.

## Metering, quota, and access logs

Eight total attempts is both the execution ceiling and ledger bound. Each entry uses fixed enums and
bounded strings, and the serialized AI-attempt portion of an access log is capped at 8 KiB. Each
entry may contain only:

- AI-service resource name and fixed provider enum;
- credential alias;
- status or fixed failure reason;
- bounded retry hint, TTFT, and usage source;
- estimated input/output tokens and estimated cost.

It never contains endpoint URLs, resolved IPs, credentials, request/response content, provider error
messages, or tool arguments.

Accounting uses an Edgion-controlled monotonic `dispatch_started` boundary: the first attempt to
write upstream request headers or body. It does not claim that a provider accepted or billed the
request.

Accounting rules:

- A pre-dispatch connect or selection failure charges zero tokens and releases any reservation.
- After `dispatch_started`, an attempt without authoritative provider usage conservatively charges
  estimated input tokens from the canonical request and marks every token/cost field `estimated`.
- The final attempt uses authoritative provider usage when present, otherwise the existing estimate.
- Each attempt reconciles its own credential reservation. AiProxy identity quota reconciliation
  charges the sum of failed-attempt estimated input and final-attempt input/output against the
  request-stable logical model.
- Cost uses the pricing table of each attempt's selected service/model; retries therefore cannot
  disappear from Edgion's estimate. It is never described as provider billing truth.

Token sums use checked `u64` arithmetic and micro-USD cost uses checked `u128` intermediates with a
bounded `u64` serialized result. Overflow fails closed before another dispatch, records the fixed
`accounting_overflow` reason, and never wraps or silently saturates quota state.

Credential alias belongs in the access log and admin runtime snapshot only. It is not a Prometheus
label. Optional token metrics remain default-off and bounded. AI-3 adds no per-transition Prometheus
counter: the existing observability decision keeps retry/failover reason and from/to attribution in
the bounded per-attempt access log, where it is durable and aggregatable without another hot-path
metric family. Existing retry-budget, circuit, and outlier metrics remain the alerting signal.

## Token-quota actions

The action is one internally tagged object under the existing `tokenQuota`:

```yaml
tokenQuota:
  # existing redisRef, key, limits, and batching fields
  onExceeded:
    type: Throttle       # Reject | Throttle | Downgrade
    maxDelay: 250ms      # Throttle only
```

For downgrade, the same field is:

```yaml
onExceeded:
  type: Downgrade
  model: chat-economy    # explicit logical model or alias
```

Unknown or cross-variant fields are rejected. The current default is `{type: Reject}`.

- `Throttle.maxDelay` is required, must be between 1ms and 5s, and is clamped to the remaining
  `HTTPRoute.timeouts.request` deadline. It waits cancellation-safely until the earlier of the Redis
  window's next admission time and that effective bound, then re-runs admission exactly once. A
  cancelled request stops the timer; deadline expiry or a second denial returns the existing quota
  error. It never holds a request or Redis permit beyond the bound.
- `Downgrade` requires an explicit target logical model, records the target in the access log, and
  adds the bounded downstream response header `x-edgion-ai-model-downgraded: <logical-model>` to
  every response produced after the downgrade. The model is validated with the normal 512-byte
  model-name bound. Downgrade is valid only when every token-quota limit uses
  `identityModel`; the downgraded model is re-admitted against its own bucket. It is invalid for an
  identity-wide exhausted bucket because changing models cannot restore a shared token budget.
- Capability mismatches on a selected fallback or downgrade fail explicitly. No tool, modality, or
  response-format feature is silently removed.

## Failure behavior

- A missing, denied, unsupported, or structurally invalid backendRef remains in the first-stage
  conformance gate. Its selected weight returns the Gateway API-required 500 and does not enter AI
  failover.
- Runtime states such as inactive credentials, ejection, concurrency saturation, credential
  exhaustion, and an unavailable requested model are classified with fixed internal reasons.
- If another runtime-eligible candidate exists, the picker continues within the route and retry
  budget. Capability conversion failures discovered after an attempt is selected fail explicitly;
  the gateway never removes request features and tries a weaker representation.
- If none exists before dispatch, return a fixed OpenAI-compatible 503.
- If no candidate remains after a provider 429, expose the normalized fixed 429 and valid
  Retry-After to the client.
- Internal topology, dependency errors, and provider bodies never enter the response.

## Code change plan

### Resource and CRD layer

- `edgion-resources/src/resources/http_route.rs`: canonical typed HTTP backend classification.
- `edgion-resources/src/resources/edgion_ai_service.rs`: credential pool schema and validation.
- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`: typed target support and
  kind-gated validation.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_proxy.rs`: remove
  `aiServiceRef`; add `backendSelection` and bounded quota actions.
- `config/crd/edgion-crd/{edgion_ai_service_crd.yaml,edgion_backend_traffic_policy_crd.yaml}`:
  structural schema updates.

### Controller

- Route handler/utilities: AI existence/status, ReferenceGrant, dependency replacement, delegated
  terminal refs, and invalid AI backend combinations.
- Controller runtime: AI-service reverse-reference manager.
- EdgionAIService handler: multi-Secret resolution, partial-good status, route and policy requeue.
- BackendTrafficPolicy handler/state: typed Service/AI-service keys, target dependencies, field
  applicability, AI ancestors, and collision-free arbitration.

### Gateway

- Route compilation: compile backend kind and target key once; preserve invalid weighted slots.
- AiService store: compile credential pools and provide a single-snapshot multi-get.
- AI routing modules: immutable attempt plan, AiProxy-selected picker, telemetry snapshot, credential
  state.
- Proxy attempt hooks: AI peer preparation plus exclusive retry arbitration at the existing status,
  connect, and timeout sites.
- Shared outlier/circuit modules: typed target identity and one half-open probe lease.
- AI usage/logging: attempt-aware processor and bounded ledger.
- Service-only consumers such as preload, BackendTLSPolicy, session persistence, endpoint validation,
  and internal-jump plugins: branch on the canonical backend kind and never treat AI refs as Services.

## Test design

### Resource and Controller tests

- Same-namespace and cross-namespace AI refs; grant add/delete self-healing.
- Missing AI service create/delete requeue and Route `ResolvedRefs` transitions.
- Exact group/kind matching, absent port, homogeneous AI rules, the two allowed backend filters, and
  rejection of every other backendRef filter type.
- Delegated child AI refs preserve child namespace and dependencies.
- Service and AI service with identical namespace/name never collide.
- EBTP Service and AI-service scopes, homogeneous targets, field matrix, partial multi-target conflict
  promotion, ancestor status, and old/new target replacement.
- Delegated AI rules preserve terminal child backend refs, namespace, and adaptive-state identity.
- Credential Secret partial resolution, rotation, removal, and zero-valid fail-close.

### Gateway unit tests

- HTTPRoute weight is the only weighted input; policy cannot inject a candidate.
- Invalid refs retain their exact aggregate traffic share under every picker and never fail over.
- Priority follows post-delegation declaration order; zero-weight refs are never selected.
- LowestLatency bootstrap/exploration, 30-second p50 publication, and state reset/retention.
- LowestUsage 60-second weighted-capacity score and retry-token attribution.
- Cost-weighted fixed-point selection and missing-price rejection.
- Canonical bytes remain unchanged across Anthropic then OpenAI preparation.
- Each attempt removes previous provider headers and publishes the correct BodyStorage snapshot.
- Cross-provider retry restores pristine post-rule headers/URI before applying the next backend's
  RequestHeaderModifier; no previous header, URI, mirror, redirect, or response modifier leaks.
- HTTPRoute retry and BackendTrafficPolicy retryConstraint do not double count.
- Credential-first, service-next, and new-round retry traversal, including a single-backend route.
- Seven retries produce eight total attempts; an eighth retry is rejected by status and runtime.
- Pre-dispatch connect retry is automatic; ambiguous AI POST replay requires the existing opt-in.
- 100/103 permit retry; 101/final header/body prohibit it.
- Exactly one half-open probe is admitted under concurrency.
- Failed-attempt usage and cost enter final quota reconciliation and access log.
- Logical aliases mapping to different provider model names share one identity-model quota key.
- Credential RPM/TPM reservations reconcile per attempt; Retry-After rotates credentials first.
- Missing, duplicate, globally attached, and backend-attached AiProxy configurations fail closed.
- No credential alias or resource name enters Prometheus labels.

### Integration tests

- Anthropic 429 to OpenAI fallback with correct OpenAI request and response shapes.
- OpenAI 503 to Anthropic fallback.
- Retry-After cooldown and route backoff behavior.
- Three SSE events followed by disconnect causes no fallback.
- Circuit open, bypass, one half-open probe, recovery, and primary return.
- Credential RPM/TPM exhaustion rotates within the service before cross-service failover.
- Dynamic route weight, policy, credential Secret, and AI service updates affect fresh requests.
- One request retains its captured destination-policy snapshot across retries while the next request
  observes a hot-reloaded EBTP without rebuilding the route table.
- Redis final usage includes failed-attempt estimated input and final authoritative usage.
- Pre-dispatch cancellation releases its credential reservation; ambiguous cancellation after
  dispatch retains the full bounded reservation until window expiry.
- An authoritative terminal result refunds only the proven-unused output reservation.

## Compatibility and rollback

The project is pre-release. `AiProxy.aiServiceRef` and the top-level AI-service `secretRef` are
removed rather than retained as aliases. Operators update HTTPRoute backendRefs and the credential
pool atomically with the CRDs and binaries.

Rollback is feature-local: an HTTPRoute can point back to an ordinary Service and remove AiProxy.
No Pingora fork change or transport migration is required.
