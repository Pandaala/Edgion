---
name: opaque-header-fidelity
description: Review ExtProc ordinary header byte projection, typed mutations, presence checks, and local-reply queues.
knowledge_type: regression-guards
---

# ExtProc opaque header fidelity

## Conclusion and scope

D0152 (`preserve-ext-proc-opaque-header-fidelity`) concerns ordinary request and
response headers and ImmediateResponse header operations. Legal HTTP header
values need not be UTF-8. Projection uses protobuf `HeaderValue.raw_value`;
mutations validate bytes into `http::HeaderValue`, and the canonical session
adapter and `PendingHeaderOp` preserve that typed value through delivery.
Presence is a header-name lookup, independent of successful text decoding.

## Rationale and evidence

The original collectors called text-only session accessors, which dropped values
rejected by `to_str`. Mutation decoding used `String::from_utf8_lossy`, replacing
opaque bytes. Conditional actions also used text access to infer presence.
Finally, `PendingHeaderOp.value: String` prevented a byte-preserving local reply.

The correction follows the existing raw-byte trailer pattern in
`edgion-gateway/src/plugins/http/ext_proc/response_trailer.rs` and these owners:

- `edgion-gateway/src/plugins/http/ext_proc/mutation.rs`: ordinary projection,
  byte validation, action dispatch, and local-reply operation creation.
- `edgion-gateway/src/plugins/runtime/traits/session.rs` and
  `edgion-gateway/src/plugins/runtime/session_adapter.rs`: canonical typed
  setters/appenders and live response-header presence.
- `edgion-gateway/src/ctx.rs`: typed pending values, replay, and drain.

Request projection borrows the existing HeaderMap. Response projection consumes
the byte accessor's owned values. Presence checks must not copy the response
map. Typed writes preserve sensitivity metadata and successful request writes
retain the adapter's deferred-credential invalidation semantics. See
[borrowed header access](../../plugin-runtime/regression-guards/borrowed-plugin-header-access.md)
and [sensitive header ownership](../../security/mandatory-design/secret-header-values-must-be-sensitive.md).

Envoy's [ExtProc API](https://github.com/envoyproxy/envoy/blob/main/api/envoy/service/ext_proc/v3/external_processor.proto)
specifies `raw_value` for projected headers. Its
[mutation utility](https://github.com/envoyproxy/envoy/blob/main/source/extensions/filters/http/ext_proc/mutation_utils.cc)
copies raw values and validates them before insertion, without UTF-8 replacement.
These upstream links are moving references, not a pinned interoperability result.

## Alternatives and limits

This correction preserves Edgion's existing non-empty raw-value precedence,
text fallback, `keep_empty_value`, four append actions, and set-before-remove
order. The Envoy implementation inspected during investigation reads `raw_value`
directly, uses the older `append` field, and removes before setting; do not claim
complete mutation equivalence or silently migrate those independent policies.

Invalid HTTP value bytes are skipped with a static PluginLog error, not repaired.
Protected pseudo/framing headers remain rejected. Repeated values retain their
per-name order; a HeaderMap does not preserve global wire order across names.
The live response-presence seam excludes queued operations when no response is
bound. Local-reply conditions run against the fresh reply when applied.

String response writes without a bound response retain their prior successful
no-op outcome for invalid values: parsing now drops them before queueing instead
of discarding them on application. With a bound response, invalid values still
return an error. Typed queues preserve an existing sensitivity flag; they do not
infer whether arbitrary processor bytes originated from a secret.

## Verification and re-evaluation

Fresh scoped verification passed 170 tests: 16 ExtProc mutation tests, 119
session-adapter tests, and 35 local-reply, streamed-response, Wasm, preflight,
request-body and failure-response tests. Coverage includes raw-map/protobuf
round trips, sequential conditions, invalid bytes, and sensitivity metadata.
Independent code review returned LGTM after the DSL Redis test session gained
the four required fail-fast trait stubs; its scenarios and assertions were not
changed. No performance benchmark or deployed wire test was run.

`cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` passed (Clippy reported warnings).
SSA, metrics inventory, Pingora source, Gateway tracing, and English-only guards
also passed. The latest agent-doc check is blocked by an
unrelated missing routing follow-up target referenced from
`routing/regression-guards/radix-path-specificity.md`; the active task remains
open until required checks pass.

The Full integration case `external_opaque_header_wire_fidelity` now exercises
fixed repeated non-UTF-8 request values, request and response typed mutations,
upstream response projection, and ImmediateResponse headers over real HTTP/1
and HTTP/2 downstream connections. The independent processor uses official Go
v1.37.0 types, validates exact protobuf `raw_value` bytes, and publishes only
fixed match booleans rather than retaining arbitrary header contents. The
downstream assertions cover presence, overwrite, append, empty values, repeated
per-name order, and absence of UTF-8 replacement bytes.

`./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare
-r EdgionPlugins -i ExtProc --full-test` passed on 2026-09-13: the 33-case
ExtProc suite, dynamic TLS-policy recovery, and the integration final guards all
passed. This is deployed HTTP/1 and HTTP/2 downstream evidence against the
repository-pinned Gateway and Go fixture; it does not add an upstream-protocol
matrix or claim global ordering across different header names.
Re-evaluate if projection encoding, header validity, session APIs, queue value
ownership, deferred credential invalidation, or conditional mutation order changes.

## Trailer status cardinality without collection

The response-trailer validator checks `grpc-status` cardinality directly through
its HeaderMap iterator. An absent value is allowed only for non-gRPC; a second
value rejects before parsing. One value retains the existing text/u16/range
checks, including accepted leading plus and zero padding. The whole-map byte and
field caps and forbidden-name checks still run first. This does not relax the
wire protocol or change the validator's established input acceptance.

Pure validator tests cover absent/single/duplicate, invalid text and numeric
boundaries in both modes, plus whole-map gates. Their allocation counter excludes
fixture construction and confirms no allocation in these validator calls.
Protobuf projection, mutation and callback lifecycle retain their ownership.
A matched trailer verdict without header mutations validates the original map
and preserves its storage; a verdict with mutations still clones a candidate
and commits it only after validation and transport representability checks.
This is not an allocation-free trailer callback claim.
Existing asynchronous tests retain atomic rejection and failure-policy coverage.

## Resolution references

- D0152 / `preserve-ext-proc-opaque-header-fidelity`: confirmed correctness defect;
  implementation authorized after scope and performance discussion.
- `verify-ext-proc-opaque-header-wire-fidelity`: Full HTTP/1 and HTTP/2
  downstream wire verification completed.
