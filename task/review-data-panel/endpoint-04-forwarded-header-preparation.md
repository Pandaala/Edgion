# Decide endpoint probe forwarded-header preparation

Status: deferred for user decision. Scope: request-local header preparation for
AllEndpointStatus fan-out, not shared outbound clients or transport policy.

## Evidence

`edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs::fan_out_requests`
computes dynamic Connection-listed names once, then every endpoint filters the
same original_headers snapshot and calls reqwest RequestBuilder::header with
borrowed strings. Each endpoint repeats header-name/value conversion and copying.
The loop runs after semaphore acquisition, latency start and client_and_url
policy preparation. A backend-specific Host is added last.

The production session adapter's request_headers returns owned text pairs from
HeaderMap iteration, omitting values that fail HeaderValue::to_str. These names
and text values are already valid; malformed forwarded input is a defensive or
custom-session case, not the ordinary adapter path. This string snapshot also
does not retain HeaderValue sensitivity metadata.

Locked reqwest 0.12.28 header() appends converted values and records conversion
errors in the builder; send then yields the existing fixed request-failed
classification. Its headers(HeaderMap) instead merges through replace_headers.
A prepared map is not automatically equivalent to the ordered append loop.

## Decision

Compare keeping the current path, filtering once into a borrowed list, and
preparing typed headers once for the invocation. Measure filtering/parsing work,
header-map and clone costs, first shared-clone metadata, and overlap between raw
and prepared owners for small and large endpoint/header counts.

Preparing before the futures are polled moves work outside the current wall
budget and before endpoint policy checks. Preparing eagerly may also do work for
endpoints that would fail policy preparation or never acquire a permit. Preserve
failure precedence or explicitly review an authorized timing-boundary change;
do not return a new global error merely because preparation failed. There is no
measured throughput benefit or selected runtime representation yet.

## Required acceptance

- Preserve forwarded bytes, duplicate field values and their order, omission of
  non-text snapshot values, per-endpoint Host, and current request-builder error
  classification. Reconcile append versus replacement semantics explicitly.
- Use canonical hop-by-hop filtering and retain inbound Content-Length/Expect
  removal for these bodyless probes. The first-Connection-only lookup is a
  [known unresolved gap](../../skills/04-review/http1/regression-guards/hop-by-hop-dynamic-stripping.md),
  not an approved permanent behavior. Track any fix separately; this performance
  task neither closes nor waives that gap.
- Preserve empty-endpoint behavior, policy failure precedence, permit cleanup,
  latency and wall-deadline ownership, cancellation and partial ordinal results.
- Preserve generation checks, client isolation, TLS, SSRF and endpoint pinning.
  Do not share clients or add a cross-request cache as part of this optimization.
- Keep credential-safe diagnostics and add no raw header/token logging. A typed
  path must explicitly assess sensitivity metadata rather than claim the current
  string snapshot already preserves it.
- Cover repeated headers, filtered headers, malformed custom-session inputs,
  policy rejection, ready-at-deadline and cancellation behavior. Measure complete
  fan-out construction and retained memory, then independently review the design.

This task is separate from endpoint-01 address ownership, endpoint-02 selection
projection and endpoint-03 response-text ownership. It adds no runtime code,
tests or performance measurements.
