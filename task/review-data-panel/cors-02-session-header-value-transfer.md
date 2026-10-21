# CORS session header value transfer

Status: deferred for user decision. No runtime change.
Source baseline: `288f980d3`.

## Evidence

Cors::set_cors_headers_via_session borrows every value from its Cow header pairs
and calls the string session setter. This path serves actual responses and
preflightContinue. Echoed Timing-Allow-Origin, reflected requested headers and
formatted Max-Age already own strings and therefore undergo another byte copy
when the adapter constructs HeaderValue. Max-Age and requested-header reflection
are emitted only for preflights. Access-Control-Allow-Origin borrows within the
pairs; its separate outer origin owner is outside this proposed transfer.

The terminating preflight builder already transfers owned values directly into
Pingora insert_header. The session path has an additional failure boundary:
PingoraSessionAdapter::set_response_header checks the protected-name policy
before parsing the value. Invalid values are ignored with Ok when no response is
bound, but return an error when a response is bound. The typed setter receives
already-valid HeaderValue and cannot itself reproduce that parsing outcome.
Moving parsing into CORS without preserving this boundary changes behavior.

This is an API contract concern, not a claim that malformed control bytes are
normally admitted as CORS request headers. The existing real-adapter regression
invalid_string_response_values_keep_live_error_and_queued_noop_behavior covers
both branches and unchanged response state.

## Decision

Decide whether a shared owned-string setter or equivalent canonical transfer
boundary earns its API, implementation and test surface, or retain current copies.
Keep any representation choice local to mutation ownership; do not add a separate
CORS validation policy, guess response binding from an empty header map, or change
the queued failure policy to simplify transfer. A default implementation that
borrows the String may preserve behavior for other implementations but does not
remove production copies unless the live adapter implements the ownership seam.

This differs from [pending queue execution](response-01-pending-header-ownership.md),
which concerns cloning retained operations during replay, and from
[Max-Age preparation](cors-01-derived-max-age-preparation.md), which concerns
formatting the number before insertion. Neither decision is approved here.

## Acceptance before implementation

Use IMPL and independently review an exact producer/adapter/trait allow-list.
Preserve protected-name rejection before value handling, live errors and queued
invalid-value no-ops, first-error propagation, ordered Set replacement and Vary
append, sensitivity initialization and header bytes/casing. Preserve the existing
queue's visibility, upstream application and later local-reply replay; do not
consume pending operations early or change body-owner framing guards.

Exercise the real adapter with valid and invalid values, protected names, bound
and unbound responses, existing fields, normal application and local replacement.
Cover CORS actual responses and preflightContinue with fixed/reflected headers,
wildcard/echoed timing, Max-Age limits, Vary and mutation failures. Preserve
preflight-only header emission and plugin continuation/log outcomes. Existing
mocks alone do not prove the real queue's failure policy.

Measure avoided value-byte copies separately from name copies, queue storage,
response insertion and replay clones. Owned String-to-Bytes conversion may retain
spare capacity and allocate metadata before validation; do not infer lower total
allocations, memory or throughput from moving a String. Add no runtime cache,
tracing, metrics or sensitive diagnostic output. Direct terminating preflights
and native Gateway API CORS need no behavioral change for this proposal.

## Sources

- `edgion-gateway/src/plugins/http/cors/plugin.rs`: build_cors_header_pairs,
  set_cors_headers_via_session, handle_preflight and handle_normal_request.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: set_response_header
  and set_response_header_value.
- `edgion-gateway/src/plugins/runtime/traits/session.rs`: response mutation APIs.
- `edgion-gateway/tests/unit/plugins/runtime/session_adapter_test/record_outcome_tests.rs`.
- `skills/04-review/cors/intentional-design/response-header-values-borrowed.md`.
- `skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md`.
