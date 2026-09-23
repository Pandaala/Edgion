---
name: shared-outbound-call-unit-tests-are-network-free
description: Use when changing the shared Gateway outbound call loop or its unit tests.
knowledge_type: regression-guards
---

# Shared outbound call unit tests are network-free

## Conclusion and scope

The `edgion-gateway` shared outbound call loop has deterministic library unit
tests. The tests drive a private transport interface with scripted responses and
use Tokio's paused clock for timeout and retry timing. They do not start an HTTP
server or open a socket. The production `outbound_call_with_body` entry point
still dispatches through `ManagedOutboundClient.http_client`, whose target,
TLS, egress, redirect, and proxy policy is resolved during client construction
and dialing.

The relevant runtime fields are `http_client` (request execution),
`resolved_target` (dial destination), `retry` (retry eligibility and delay),
`timeout`/`timeout_template` (per-attempt deadline), and
`max_response_bytes` (response cap). The tests preserve the public
`OutboundCallError` classifications and completed-response contract.

## Rationale and evidence

The prior test module in `plugins/http/common/outbound/call.rs` started 20
`wiremock::MockServer` instances. It substituted `localhost` for a loopback IP
literal, then connected through `reqwest`. That required network access in a
library unit-test binary, contrary to `skills/05-testing/00-unit-testing.md`.

The production call path passes `client.http_client.as_ref()` to the private
generic loop. Its `OutboundTransport` implementation invokes the same
`reqwest::Client::execute` operation. `call_transport_tests.rs` supplies a
test-only implementation that records each request and returns scripted
headers, status, body chunks, delays, or transport errors. The response-body
collector is shared with the existing `stream_body_capped` consumers; the
production response still streams chunks under the same cap.

## Alternatives and limits

Moving the 20 cases to socket-based integration tests would not provide
network-free unit coverage of request replay, retry classification, timing,
streaming caps, framing, and cancellation. The transport interface is private
and statically dispatched, so it does not add a client field or dynamic dispatch
to the production request path.

These unit tests verify the call-loop policy and request construction. They do
not prove that a real dial obeys the managed client's TLS and final-dial egress
configuration; those remain separate integration and client-construction
boundaries.

## Verification and re-evaluation

The replacement tests retain the names of all 20 former network-backed cases
and add exact/overflow chunk caps with absent and false Content-Length values,
stream error, connect retry backoff,
framing, date parsing with a fixed clock, and cancellation checks. Re-evaluate
this guard if the call loop changes its transport, response type, or clock
handling, or if a unit test reintroduces a listener or live HTTP request.

Verification on 2026-09-23: the focused outbound-call selection passed 42
tests. Focused ForwardAuth, AI Guard, Guardrail response, and webhook resolver
selections passed 7, 6, 17, and 9 tests respectively. Workspace format, check,
and Clippy passed, as did agent-doc validation, the SSA and metrics guards,
Pingora source policy, and the Gateway API transform contract check. This
does not replace a real-network integration test of the managed client.

## Resolution references

- `isolate-shared-outbound-call-unit-tests-from-network` — selected task in
  `tasks/todo/`, resolved by the private transport test seam and deterministic
  replacement coverage.
