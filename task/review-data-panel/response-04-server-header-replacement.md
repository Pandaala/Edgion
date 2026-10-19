# Server header replacement without prior removal

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `15ff33f38`. No runtime change.

## Evidence

`edgion-gateway/src/runtime/server/server_header.rs::ServerHeaderOpts::apply_to_response`
removes each prepared field before inserting its configured case and value.
The current default entries are Server and Strict-Transport-Security. The locked
Pingora fork (`1aaa45c`) implements insertion by replacing both the case map and
value map entries, including repeated values. Omitting the preceding removal
would avoid its lookups and mutations, but is not byte-for-byte equivalent.

A standalone Rust probe linked the current `pingora_http` build artifact. It built
one response with Server, X-One, Strict-Transport-Security and X-Two in that order,
then appended a second Server value. One clone used remove-then-insert for Server
and HSTS; the other used direct insertion with the same final names and values.
The final HeaderMaps compared equal, including replacement of repeated Server
values. `ResponseHeader::header_to_h1_wire` produced different field ordering:

- Existing path: X-Two, X-One, Server, Strict-Transport-Security.
- Direct insertion: Server, X-One, Strict-Transport-Security, X-Two.

This is a reproducible observation on the locked build, not a general guarantee
of HeaderMap iteration order or evidence of a protocol violation. No CPU,
allocation, throughput or H2 compression benefit was measured. The candidate
saves operations in source; actual workload benefit remains unproven.

The helper is shared by `pg_response_filter`, `pg_upstream_response_filter`,
`pg_request_filter`, `pg_fail_to_proxy`, `runtime/server/error_response` and
`ai/http/attempt`. Its impact extends beyond the ordinary upstream response.
The synchronous normalization path restores gateway headers after provider
processors can rebuild them. Listener-selected HSTS options remain authoritative;
see `skills/04-review/backends/regression-guards/hsts-on-plain-http-error-responses.md`.
Removing HSTS from the options is not a new instruction to delete upstream HSTS.

## Decision and acceptance

Decide whether the observable H1 ordering change is acceptable for a measured
reduction in replacement work. Until then retain the existing helper. A future
IMPL must use an exact allow-list and independent plan and final reviews.

Compare absent, existing and repeated values, mixed original casing, configured
case preservation, TLS and plain-listener options, ordinary and local/error
responses, and repeated application. Preserve field values, multiplicity,
sensitivity, provider-reset ordering, response onion order and final writer
ownership. Check real H1 serialization and H2 behavior separately; map equality
alone does not establish wire equivalence. Account for response-cache header
sealing/charging without changing freshness, replay or terminal publication.
Measure lookup/CPU costs and allocations with representative response headers;
do not add hot-path tracing, logs or metrics for this investigation. Do not
introduce an alternative header map or change the Pingora dependency.

Existing response-filter tests are baseline evidence only. They do not accept
the proposed ordering change or establish its performance value.
