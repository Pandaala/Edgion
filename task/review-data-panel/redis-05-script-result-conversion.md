# Redis script result conversion storage

Status: deferred representation and performance decision for the user.
Source baseline: `5447bba14`, with locked Fred 10.1.0. No runtime change.

## Current evidence

Gateway RedisRateLimiter::eval_script requests Vec<i64> from EVALSHA and its
exact NOSCRIPT EVAL fallback. Fred's commands/impls/lua.rs first converts the
response frame into Value with protocol/utils.rs::frame_to_results; the public
Lua interface then invokes the requested FromValue conversion. Array frames are
consumed into a Vec<Value>, whose elements are subsequently converted to i64.
This is not a clone of all response payload bytes. Source-level collect calls
do not establish exact allocations: iterator collection may reuse backing storage.

The Gateway then validates exactly four rate-limit fields, five cumulative-quota
fields or seven credential-reservation fields, followed by status, numeric and
cross-field invariants. Its existing from_lua_result tests start with &[i64];
they do not exercise Fred's preceding frame/value conversion.

Fred's modules/response.rs implements FromValue for [T; N] by first converting
to Vec<T> and then try_into. Merely changing the requested result to an array
does not remove this Vec conversion. It would also move length-error production
from the Gateway parser to Fred and its EVALSHA/EVAL error wrapper.

The conversion contract is wider than a flat RESP integer array: i64 accepts
numeric strings, floating-point casts and single-element array unwrapping.
Vec conversion also has scalar, null and map branches. Element conversion runs
until the first conversion error before Gateway shape validation, so an invalid
excess element can produce a conversion error instead of a field-count error.
These are observations of current code, not a recommendation to broaden numeric
acceptance or a claim that the shipped Lua scripts normally emit these shapes.

## Alternatives and acceptance

Keep the current conversion, or separately evaluate a private inline result
representation that delegates scalar conversion to the canonical Fred behavior.
Avoid creating a competing generic Redis conversion framework. Assess whether
the potential reduction in final result storage warrants added code and larger
stack/future state. A custom representation does not itself remove the upstream
decoded frame or Vec<Value>. No allocation or throughput gain is measured here.

An approved design must preserve or explicitly seek a separate behavior decision
for every currently accepted shape and conversion failure. In particular:

- Differentially compare the existing Vec<i64> path with valid four/five/seven
  field responses, empty/short/long arrays, malformed excess fields, numeric
  strings, doubles, nested singleton arrays, null, scalar and map values.
  Include integer boundaries and feature-dependent nil conversion behavior.
- Preserve conversion-versus-length-versus-domain validation order, error kinds
  and existing EVALSHA/EVAL context. Do not silently make integer-only parsing
  or early arity rejection part of a performance change.
- Preserve every result status, safe-Lua-integer bound, signed delta, remaining
  capacity check and caller failure policy. Keep exact NOSCRIPT-only fallback,
  frozen arguments, original client generation and deadlines. Add no retry or
  refund after ambiguous execution or partial multi-policy debit.
- Compare allocation, retained capacity and stack/future size for actual script
  result sizes. Preserve cancellation cleanup and account for both decoded
  storage and the final result; do not claim a wire-decoder improvement from a
  Gateway-only container change.

This decision is separate from request-side [fallback snapshots](redis-02-fallback-command-ownership.md),
[pool selection](redis-03-pool-member-selection.md) and
[command encoding](redis-04-command-frame-encoding.md). Keep the canonical shared
executor, sensitive-data redaction and access-log ownership. Add no hot-path
tracing or new metrics. Existing integration fixtures remain unchanged; any
script-cache experiment must use an exclusively owned Redis server.

## Original knowledge and verification limits

- [Script cache lifecycle](../../skills/04-review/rate-limit/regression-guards/redis-script-cache-lifecycle.md)
- [Named quota result and partial-debit boundaries](../../skills/04-review/rate-limit/regression-guards/redis-named-quotas.md)
- [Existing Redis failure policy](../../skills/04-review/rate-limit/intentional-design/redis-fail-open.md)
- Gateway source: plugins/common/redis_rate_limiter/mod.rs, eval_script and the
  three from_lua_result implementations; associated unit tests under tests/unit.
- Fred sources: commands/{impls,interfaces}/lua.rs, protocol/utils.rs and
  modules/response.rs.

This is a source audit. No Rust tests, malformed-response fixture, allocation
probe, live Redis or benchmark ran. Socket decoding, reader dispatch and
reconnect execution are not proven by this result-conversion inspection.
