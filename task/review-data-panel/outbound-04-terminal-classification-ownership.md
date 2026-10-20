# Outbound terminal success-classification ownership

Status: deferred runtime-result ownership decision. No runtime change.
Source baseline: 2ff600beb.

## Evidence and scope

The shared loop in `plugins/http/common/outbound/call.rs` calls the managed
client's classify_detail after receiving a capped response. It uses SuccessOutcome
for body-retry eligibility and the terminal body-predicate failure metric, then
returns the completed OutboundResponse. ForwardAuth::handle_outbound_response
calls classify_detail again with the same client; resolve_webhook_key likewise
calls client.classify on the returned response before extraction.

The resource-layer canonical classifier in
`edgion-resources/src/resources/common/success.rs` first checks status. When
status is accepted, body predicates are configured and the media type is JSON,
it parses the body into serde_json::Value and evaluates JSON Pointers. In that
case these two consumers repeat JSON parsing and predicate evaluation. Status-only
and rejected-status paths do not incur that JSON parse. Invalid JSON can also be
parsed and rejected twice. No measured workload or throughput gain is established.

AI Guard and Guardrail perform their own response-protocol checks and decoding;
these are not duplicate calls to this success classifier. Webhook JSON extraction
is a separate semantic operation, also outside this candidate. Do not retain a
parsed JSON tree across consumers merely to avoid parsing; that would introduce
a different heap and lifetime tradeoff.

## Decision alternatives

Keep recomputation if its observed cost does not justify changing the result API,
or return an immutable classified result carrying the original response and the
scalar outcome from the same captured client policy. A deliberate classified-result
boundary could make reuse safe without extending parsed-body lifetime.

OutboundResponse currently exposes mutable public status, headers and body, and
can be constructed by tests or other internal callers. An unkeyed cached outcome
on that type can become stale after mutation or be interpreted against another
client policy. Decide ownership and construction invariants before implementation;
do not bolt on an optional cache, silently trust fabricated outcomes, or preserve
redundant legacy APIs as compatibility shims. Keep schema/conf_sync unchanged and
reuse the canonical SuccessOutcome/classifier. No global cache or new metrics.

## Acceptance for a later implementation

Reuse only the classification of the exact terminal response and immutable client
policy. Retried intermediate responses must not leak their classification into
the returned result. Preserve status-retry versus body-retry distinctions and the
Retry-After over-cap stop precedence. Keep the predicate-failure metric terminal
and exactly once, and passive health status-based. Do not move transport accounting
into consumers or change caps, deadlines, diagnostics, body ownership or errors.

ForwardAuth must retain BodyFail-to-403 mapping, genuine rejected status relay,
authentication delay, selected header protection, bounded denial body and transport
failure behavior. Webhook must retain status_error versus call failure outcomes
and existing extraction semantics. Direct helper/test construction needs an explicit
safe path rather than a default success value. Existing sensitivity and redacted
Debug boundaries remain in force.

Measure representative status-only, rejected-status, valid/invalid JSON and
predicate-count cases. Count classification/parsing invocations through the actual
loop-to-consumer path, including retry then terminal success/failure. Exercise old
client generations retained through hot reload and prove no outcome can be reused
with mutated bytes or another policy. Verify all four shared outbound consumers,
header-only collection, resource predicate tests and project checks under an
independently reviewed IMPL plan with an exact implementation allow-list.

Original knowledge: skills/01-architecture/02-gateway/request-template-runtime.md,
skills/04-review/testing/regression-guards/shared-outbound-call-unit-tests-are-network-free.md,
and skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md.

Fresh baseline verification passed 123 shared outbound, 13 ForwardAuth, 47 Webhook
and 20 Resources success-predicate tests (203 total), formatting, workspace
all-target check/Clippy and agent-doc validation. Existing unrelated warnings
remain. No classification-count or throughput benchmark ran; these tests verify
current behavior, not a proposed result representation.
