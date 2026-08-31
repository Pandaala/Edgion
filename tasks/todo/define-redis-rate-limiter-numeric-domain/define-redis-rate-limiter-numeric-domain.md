# Define the Redis Rate-Limiter Numeric Domain

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Define one shared exact numeric domain for the HTTP RateLimitRedis and stream ConnectionRateLimit policies before values enter Redis Lua arithmetic, reply conversion, or the stream local gate. Accepted `u64` values can currently exceed Lua's exact integer range, Redis signed-integer operations, or local rate×interval/time conversion bounds, turning configured limits into rounded values, overflow, or runtime errors that then follow fail-open policy.

## Requirements

- Own the canonical rate-limiter domain constants and validation helpers in `edgion-resources`, or another DAG-lower leaf selected explicitly in `01-design.md`, and import them into the Gateway defense. Do not create a duplicate Gateway constant or reverse the workspace dependency DAG.
- Cover HTTP `RateLimitRedisConfig.policies[*].rate`, `policies[*].interval` and compiled milliseconds, plus `RateLimitRedisConfig.cost`.
- Cover stream `ConnectionRateLimitConfig.perSourceIp.{rate,interval}` on the Redis path and `perListener.{rate,interval}` on both its unconditional local gate and optional `redisDistributed` path. `redisTimeout` is not a Lua numeric input and remains outside this domain contract.
- Account explicitly for Lua double exact-integer limits, Redis `i64`/`INCRBY` behavior, response decoding into Rust integer types, millisecond conversion, TTL multiplication, retry arithmetic, and checked local multiplication.
- Bound and check `LocalRateGate::{new,update_params,refill}` arithmetic, including `rate * 1000`, capacity delta/addition, `elapsed * rate * 1000 / interval`, and every `u128` to `u64` conversion for both local-only and distributed per-listener configurations.
- Reject out-of-domain configuration during resource validation with stable field-specific English errors and matching schemars/CRD constraints where the resource architecture supports them. If schemars needs a literal bound, add a parity guard against the canonical constant.
- Retain a Gateway-side defensive check before Lua evaluation so malformed or bypassed conf-sync input cannot reach an inexact script invocation.
- Preserve algorithm behavior, key/hash-slot construction, atomic Lua execution, EVALSHA/NOSCRIPT fallback, failure-policy defaults, headers, metrics, and logging/redaction.
- Do not place canonical constants in a higher-level crate that would reverse the `{resources → common → gateway}` dependency direction.

## Scope

### In scope

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/rate_limit_redis.rs`
- `edgion-resources/src/resources/edgion_stream_plugins/connection_rate_limit_config.rs`
- Gateway shared Redis rate-limiter defensive validation and affected HTTP/stream caller preparation.
- `edgion-gateway/src/plugins/stream/connection_rate_limit/local_gate.rs` and its local-only/distributed configuration updates.
- Schemars/CRD coverage and canonical EN/ZH feature/operator documentation when applicable.
- Focused resource, Gateway core, HTTP, and stream tests for exact boundary, boundary plus one, and derived-overflow cases.

### Out of scope

- Cumulative-quota and credential-reservation APIs, which have separate bounded contracts.
- RedisLinkClient connection, lifecycle, DNS, TLS, egress, or pool behavior.
- Changing rate-limit algorithms, keys, hash slots, failure policies, response headers, metrics, or Lua reply shapes.
- Modifying existing `edgion-tests/**` files.

## Acceptance criteria

- One DAG-lower canonical contract, imported by Gateway with no duplicate constant, prevents every accepted rate, cost, interval, TTL, timestamp, counter, local-gate capacity/refill, and derived product from losing integer exactness or exceeding its Rust/Redis representation.
- HTTP and stream resource validation reject the exact first invalid value and retain valid maximum values; generated schema constraints match the chosen domain and a parity guard prevents literal drift.
- Gateway defense rejects malformed/bypassed values before any Redis command and routes the fixed internal class through the existing failure policy without exposing numeric internals.
- Lua and Rust boundary tests cover exact maximum, maximum plus one, multiplication overflow, interval-to-millisecond conversion, TTL multiplication, and reply decoding.
- Existing FixedWindow, SlidingWindow, TokenBucket, local-only and distributed per-listener, per-source-IP, key/hash-slot, failure, header, metric, and redaction tests remain green.
- Existing integration tests remain unchanged; the separate real-Redis partial-refill task covers B0061's behavioral regression rather than numeric-domain design.
- Formatting, workspace check, Clippy, CRD/schema guards when applicable, documentation validation, metrics inventory, and relevant repository guards pass.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `define-redis-rate-limiter-numeric-domain.md` | done | Analysis boundary and acceptance contract |
| `00-context.md` | not created | Create before implementation |
| `01-design.md` | not created | Required before implementation |
| `02-implementation.md` | not created | Create after independent design approval |
| `03-subtasks.md` | not created | Required with the approved cross-module plan |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer from B0061 as a cross-resource numeric-contract task. | A Gateway-only guard would leave invalid configuration admitted, while resource-only validation would not defend bypassed input. |
