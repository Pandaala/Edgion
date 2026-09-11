---
name: debug-access-log-development-surface
description: Use when reviewing the size, schema, and failure behavior of the client-visible debug access-log header.
knowledge_type: intentional-design
---

# Debug access-log response header: accepted development-only limitations

## Conclusion and scope

On 2026-09-08, the user explicitly chose not to implement D0088 and requested
closure after discussing the controlled debugging use case and possible response
failure from oversized headers. Retain the existing `DebugAccessLogToHeader`
behavior. This accepts a real robustness limitation; it does not classify the
missing serialized-byte bound as fixed or a false positive.

The acceptance covers this optional development/debugging response surface only.
Development-only use is operator discipline documented in the user guide, not an
enforced build or runtime restriction. It grants no production reliability
guarantee and makes no claim about maximum local access-log record size.

## Rationale and evidence

- `edgion-resources/src/resources/edgion_plugins/plugin_configs/debug_access_log.rs`:
  `DebugAccessLogToHeaderConfig` is empty; enabling the plugin selects this surface.
- `edgion-gateway/src/plugins/runtime/gateway_api_filters/debug_access_log.rs`:
  `add_debug_header` constructs `AccessLogEntry::from_context`, calls `to_json`,
  and discards the result of `set_response_header`. No total serialized-byte cap
  or dedicated response schema exists.
- `edgion-gateway/src/observe/access_log/entry.rs`: `request_info`,
  `backend_context`, and `stage_logs` borrow current request state. `to_json`
  returns fallback JSON on serialization error, rather than omitting the header.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: the setter either
  inserts into the current response or queues a pending header operation.
- [Canonical snapshot and masking contract](../../../03-coding/observability/00-access-log.md)
  remains applicable: no `ext` or `waf`, unfinished terminal facts, Replace
  masking, explicit Off exposure, and B0125's early Wrap suppression.
- [User guide](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/debug-access-log.md)
  already limits recommended use to development and warns about response inflation.

HTTP defines no universal per-field size limit
([RFC 9110 section 5.4](https://www.rfc-editor.org/rfc/rfc9110.html#section-5.4)).
A 32 KiB or larger header can work on a suitably configured chain, but this is
not a portable allowance: recipients and intermediaries can impose smaller limits,
including limits on the entire uncompressed field section
([RFC 9113 section 4.3](https://www.rfc-editor.org/rfc/rfc9113.html#section-4.3)).

## Alternatives and limits

The benefit is avoiding a dedicated schema, protocol budget, and failure-policy
project for a low-frequency debugging convenience without a reproduced failure.
The accepted costs are oversized snapshots potentially disrupting a debug response,
silent header-set failure, serialization fallback instead of omission, and a schema
coupled to a non-terminal general access-log snapshot. Collection-level limits do
not prove a bound on the complete JSON. No JSON truncation is introduced.

A compact schema, hard byte cap, enforced development-only gate, and plugin removal
were considered as the task's alternatives; none is implemented by this closure.
This decision does not weaken masking, response streaming, binary-owned logging,
or canonical response-header policy. Shared response-header mutation work remains
a separate concern.
[D0087 wire coverage](../regression-guards/debug-access-log-wire-and-lifecycle.md)
verifies the masking and lifecycle contract separately; it does not establish
a response-header byte bound.

## Verification and re-evaluation

Current source and the two existing plugin unit tests were inspected. Those tests
cover early Wrap suppression and parseable non-Wrap header wiring, not byte limits,
failure injection, or cross-process protocol behavior. No runtime code or integration
tests change in this closure; no oversized-header failure was reproduced.

Closure checks on 2026-09-08 passed: `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`
(with warnings in unchanged Rust code), and the agent-doc, SSA-force, metrics
inventory, and Pingora source-policy guards. No unit or integration tests were run
for this documentation-only decision; these checks do not prove wire-size safety.

Re-evaluate on a concrete debugging failure, production-support requirements,
material snapshot growth or new sensitive fields, or a D0068 change affecting this
setter. Any future fix must compose with the shared response-header mechanism and
preserve B0125 masking boundaries; this acceptance is not a blanket exemption.

## Resolution references

- D0088, discovered in B0125 on 2026-09-08: originally medium / needs-discussion;
  closed as an explicitly accepted debugging limitation without implementation.
- Former task: `tasks/todo/define-bounded-debug-access-log-response-surface.md`.
