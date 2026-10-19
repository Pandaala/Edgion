# Evaluate single-line tracestate ownership

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `81958c2cd`.

## Evidence and alternatives

request_trace_id/mod.rs::initialize validates tracestate only after accepting a
single valid inbound traceparent. validated_tracestate copies even one physical
header line into a combined String, validates its members, then moves that String
into HeaderValue. Reconciliation later clones the retained value into upstream
headers; it does not automatically echo tracestate to the response.

Evaluate validating a single physical line through borrowed text and retaining
its original HeaderValue by clone, with the existing concatenation for multiple
lines. Share the member-validation logic rather than creating another parser.
This could remove the single-line byte copy, but retains shared storage and
reference-count work. It does not remove member-key bookkeeping or validation.

The pinned Pingora fork at 1aaa45cc70430bc782990bc20ddee3fef36d6b99 builds HTTP/1
request HeaderValues from slices of a frozen input buffer in
pingora-core/src/protocols/http/v1/server.rs; utils::KVRef::get_value_bytes uses
Bytes slicing. A small value can therefore share a larger allocation. However,
accepted traceparent is already cloned into the same request-owned W3C context,
and other request headers can retain that allocation too. An incremental memory
regression from this proposal is not established. Inspect actual owner lifetimes
and HTTP/2 construction before selecting a representation. The existing combined
String also has capacity and ownership metadata costs; neither choice is a
universal memory or throughput improvement.

## Required boundaries

Preserve sensitive-value and non-ASCII rejection, the 512-byte combined limit,
32 nonempty-member limit, duplicate-key rejection, key grammar and 256-byte
member-value limit. Trimming is for validation only: accepted wire bytes, member
order and whitespace stay unchanged. Preserve empty-list-member acceptance,
empty-line separator behavior, all-empty versus OWS-only distinctions and the
empty combined value returning None. Keep the multi-line path equivalent,
including invalid later lines and boundary accounting.

Keep capture conditional on valid inbound traceparent, request-owned identity,
upstream reconciliation, default-header suppression and access-log masking.
Do not introduce protocol normalization, response echo, caches, logging, metrics
or an alternate parser. Canonical lifecycle authority is
skills/04-review/observability/intentional-design/request-trace-id-lifecycle.md.

## Acceptance for a later implementation

Compare allocation calls/bytes, CPU and retained backing memory for absent,
single-line and multi-line headers, short/maximum-sized valid values, invalid
inputs, and short/long requests under HTTP/1 and HTTP/2. Account for existing
traceparent and request-header owners and cleanup after header replacement.
Exercise exact wire preservation, duplicate and empty members, sensitivity,
all limits, later invalid lines and forwarding through production adapters.

Fresh request_trace_id selection passed 29 tests for the current implementation.
No candidate implementation, allocation benchmark or live transport run occurred.
This decision is separate from the completed owned-String-to-HeaderValue transfer
and empty-prefix RequestId generation changes.
