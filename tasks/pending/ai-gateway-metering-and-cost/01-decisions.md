# Decisions and Invariants

## Product boundaries

- Pricing belongs to canonical `EdgionAIService.spec.models[]`; aliases reuse canonical pricing.
- Token quota is optional nested `AiProxy` configuration. A separate quota plugin was rejected
  because it would depend on fragile ordering and duplicate provider/model/stream parsing.
- Existing authentication plugins establish identity. Quota only reads configured `KeyGet`
  sources and hashes the composed identity before Redis key construction.
- Virtual keys are deferred. Usage observation through the durable access log is the transition
  path; no second authentication or persistent admin store is introduced.
- Redis is replaceable enforcement state. The access log is the durable usage/cost ledger.

## Observability

- The existing optional/default-off bounded GenAI token counter remains unchanged.
- No model label, cost metric, TTFT histogram, time-per-token histogram, or audit sink is added.
- Prices and cost arithmetic are fixed-point; no floating-point value enters configuration or the
  access log calculation.
- Cache-read and cache-write input usage remain distinct. Missing specialized prices fall back to
  ordinary input pricing.

## Execution invariants

- The synchronous response-body hook performs only bounded parsing, atomic updates, and coalesced
  `Notify` wakeups. It never awaits Redis, logs per chunk, or spawns per chunk.
- Each quota-enabled request owns one bounded task. Threshold and timer wakeups serialize through
  that task, so Redis updates remain ordered.
- OpenAI-compatible streaming uses semantic content/tool delta events as an execution estimate.
  Anthropic uses the same bounded delta estimate until its cumulative provider output usage arrives.
  Final access-log usage always uses provider totals when available and otherwise the existing marked
  estimate.
- Streaming provider usage objects are cumulative snapshots. Repeated snapshots merge by monotonic
  component high-water marks, so continuous-usage compatible providers can refine an early zero and
  a lower or out-of-order frame cannot refund usage already observed.
- Every Redis mutation names an internal UUID, monotonically increasing revision, and cumulative
  target. Retries are idempotent; final usage may apply a signed correction.
- The quota window is frozen at request start. A constant-size bucket and per-request marker share
  an explicit Redis Cluster hash slot. Successful usage updates renew an active-request lease; when
  no usage changes, one sparse half-lease heartbeat renews the same cumulative target. Finalization
  stops renewal and installs bounded cleanup TTLs. Window selection uses the gateway wall clock, so
  operators must keep gateway nodes time-synchronized around fixed-window boundaries.
- Quota-policy Redis identity includes the stable scope and window duration, but not list position
  or token limit. Reordering rules cannot exchange buckets, while changing only a limit preserves
  consumption already accumulated in the current window.
- Generated usage is force-charged past the limit. Until AI-4, the active response continues and
  subsequent admission rejects the debt.
- Missing identity and Redis failure default to deny. Explicit allow remains available under the
  existing frozen `OnMissingKey` and `OnRedisFailure` vocabularies. An allowed missing key is logged
  as `bypassed`; an allowed Redis failure records a fixed fail-open outcome and `quota_status`.
