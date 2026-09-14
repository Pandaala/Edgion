---
name: dsl-runtime-limit-scalar-contract
description: Use when reviewing DslConfig runtime-limit defaults, ranges, Controller/Gateway parity, or standalone DSL factory validation.
knowledge_type: regression-guards
---

# DSL runtime-limit scalars use one resource-owned contract

## Conclusion and scope

Every top-level `DslConfig` runtime-limit scalar is validated and
default-resolved by `DslConfig::resolve_runtime_limits` in `edgion-resources`.
`PluginValidation::validation_error`, `DslConfig::compile_and_validate`, and
both standalone Gateway factories reject the same zero or over-limit value
before publishing `VmLimits` or `DslRuntime`.

The accepted ranges are:

| Field | Default | Range |
|---|---:|---:|
| `maxSteps` | 10,000 | 1–1,000,000 |
| `maxLoopIterations` | 100 | 1–100,000 |
| `maxCallCount` | 500 | 1–100,000 |
| `maxStackDepth` | 128 | 1–1,024 |
| `maxStringLen` | 8,192 | 1–1,048,576 |
| `maxListLen` | 1,024 | 1–16,384 |
| `maxMapSize` | 512 | 1–4,096 |
| `maxBodyScanPasses` | 4 | 1–16 |
| `httpMaxTimeoutMs` | 5,000 | 1–60,000 |
| `httpMaxResponseBodyBytes` | 1 MiB | 1 byte–1 MiB |
| `httpMaxRequestBodyBytes` | 1 MiB | 1 byte–32 MiB |

Zero is not a capability-disable value. Nested Redis rate-limit policy fields
retain their separate resource-owned policy validation because their rate,
window, and timeout form one atomic algorithm contract rather than a top-level
VM/runtime scalar.

For `http.call`, `maxMapSize` bounds only the nested response-header Map. Header
processing retains the raw traversal prefix until that Map contains exactly the
configured number of names, then ignores the tail. The fixed outer result envelope
always contains `status`, `body`, and `headers`, even when `maxMapSize` is below
three; applying the collection limit to that protocol envelope would make the
documented result shape unusable.

## Rationale and evidence

The canonical sandbox model requires a finite product of instruction, call,
string, collection, body, and external-call limits. See
[`dsl-engine-security-model.md`](../../../01-architecture/02-gateway/dsl-engine-security-model.md)
and the repository HTTP-body invariant for separately bounded outbound
exchanges.

Before this correction, `DslConfig::validation_error` checked only a subset of
the scalar fields, while `DslConfig::compile_and_validate` checked none of
those ranges. `DslPlugin::resolve_parts` called the latter and copied raw
operator values directly into `VmLimits` and `DslRuntime`. Consequently,
standalone request/response factory calls could bypass even existing checks,
and normal resource validation accepted unbounded List, Map, HTTP timeout, and
HTTP response-size configurations.

`httpMaxTimeoutMs` reuses
`resources::common::external_http::MAX_OUTBOUND_TIMEOUT_MS`; it does not create
a second outbound timeout ceiling. The 1 MiB response maximum matches the
existing effective `min(httpMaxResponseBodyBytes, maxStringLen)` ceiling,
because `maxStringLen` itself cannot exceed 1 MiB. The List and Map maxima
allow substantial growth over their defaults while preventing a bounded input
from authorizing millions of independently allocated collection elements.

## Alternatives and limits

Clamping invalid configured values was rejected because it hides operator
mistakes and would make Controller status differ from the submitted resource.
Keeping range checks only in generic plugin validation was rejected because
the Gateway factories are independent trust boundaries. Duplicating defaults
or bounds in Gateway types was also rejected; their defaults now refer to the
resource constants.

This contract does not replace stricter consumer bounds: `range` still has a
1,000-element ceiling, request Body uploads still use the gateway physical
body cap, response conversion still uses `maxStringLen`, and bytecode retains
its compile-time structural limits. It is not an absolute size guarantee for
every collection returned by an API: the fixed three-field top-level HTTP
result Map and Redis result envelope retain their documented schemas, while a
five-field `redis.rate_limit` decision requires `maxMapSize >= 5` and
header-name Lists use their independent 256-element hard cap. It does not
change DSL error policy, response streaming, the outbound client, or Tokio
bridging.

## Verification and re-evaluation

Verified on 2026-09-13:

- Resource-focused DSL configuration tests passed 19/19. They cover default
  resolution, exact minimum/maximum values, zero, maximum-plus-one,
  `u64::MAX` timeout handling, and dual-fault ordering.
- Gateway DSL factory tests passed 6/6; the dedicated boundary case drives
  all eleven invalid scalars through both standalone factories.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy retained existing
  workspace warnings.
- Agent-doc validation, SSA-force, metrics-inventory, Pingora-source, and diff
  checks passed.
- Independent read-only review approved the final code, range rationale,
  deterministic validation ordering, bilingual docs, and knowledge scope.
- No full `cargo test --all` or end-to-end integration run is claimed; the
  source task explicitly excluded changes under `edgion-tests/**`.

Re-evaluate the ranges if `Value` collection representation, the maximum DSL
string size, outbound response aggregation, gateway physical request-body
policy, or the shared outbound timeout contract changes. A new runtime-limit
field must join `resolve_runtime_limits`, its boundary tests, both user-doc
languages, and its `VmLimits` or `DslRuntime` consumer.

Verified on 2026-09-14 for the `http.call` Map boundary:

- Synthetic in-memory responses passed focused tests for retaining exactly the
  `maxMapSize` response-header prefix and for preserving all three outer result
  fields when `maxMapSize = 2`.
- The DSL sandbox test selection passed 9/9 after removing its two Wiremock-backed
  cases. Those unit scenarios no longer listen or dial; the existing end-to-end
  integration bytes were not changed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy retained the existing
  `edgion-tests` `redundant_async_block` warning.
- Agent-doc validation, SSA-force, metrics-inventory, and Pingora-source guards
  passed. No full `cargo test --all` or end-to-end integration run is claimed.

## Resolution references

- `bound-dsl-runtime-limit-scalars.md` (created 2026-09-10, P1 bugfix task):
  fixed by the resource-owned scalar contract and Gateway factory parity on
  2026-09-13.
- `isolate-dsl-sandbox-unit-tests-from-network.md`: moved the two response-Map
  boundary scenarios to synthetic responses without changing their contracts on
  2026-09-14.
