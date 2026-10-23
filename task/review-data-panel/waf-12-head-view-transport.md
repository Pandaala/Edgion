# WAF head descriptor transport

Status: deferred for user decision. Source baseline: `f523c9c30`.
No runtime change or measured performance gain is claimed.

## Evidence and decision

In `edgion-gateway/src/plugins/http/waf/mod.rs`, request_header_views collects
RequestHeader descriptors into a Vec. Each descriptor borrows name/value bytes;
the collection does not copy header payloads. Capacity includes an appended Host
view only for HTTP/2.0 with no explicit Host and an available URI authority.
The response-header callback similarly collects borrowed ResponseHeader views.
The current code drops these arrays after successful transaction publication.
That shortens their lifetime but leaves their construction allocation.

RequestHead and ResponseHead in `edgion-waf/src/seclang/transaction.rs` accept
slices. RequestState::build checks the complete count, fixed values and headers,
then computes query/cookie counts and derived/retained projections before owned
materialization. ResponseState::build checks status, count and every header before
allocating owned entries. Both paths require stable repeated access. The
transaction must own its published bytes independently of the live session.

Possible approaches are an inline descriptor container with heap spill, or a
repeatable borrowed traversal interface. Neither is selected. Inline storage
trades allocation frequency for stack/container size; measure the generated
request future rather than assuming an array survives an await. A traversal
interface changes the leaf API and its multi-pass contract. A one-shot iterator
or an early capped collection is not an equivalent replacement.

This is distinct from [response-head arena storage](waf-04-response-header-arena.md),
which concerns the transaction's owned payload representation and explicitly
excludes Gateway adapter redesign. Removing a descriptor array does not remove
the required owned payload copies. Do not import HeaderMap or Gateway runtime
dependencies into the leaf to simplify this adapter.

## Acceptance for an approved implementation

- Preserve full counts, duplicate iteration order, empty and permitted non-UTF-8
  values, and the exact existing HTTP/2 authority-to-Host synthesis conditions.
  Do not mutate live headers or normalize additional fields.
- Preserve stable bytes/order across every traversal, complete validation, actual
  counts in errors, request count-before-fixed-value precedence, response
  status-before-count precedence, invalid-field ordinals, and subsequent budget
  and allocation ordering. Review removed allocation failure opportunities
  explicitly; do not truncate to a configured maximum.
- Preserve transaction ownership after source buffers disappear, phase guards,
  repeated-publication rejection, and failed publication leaving state absent.
  Keep logical retained charges and request/response body ownership unchanged.
- Compare common, empty, large, duplicate and invalid heads across the complete
  callback path. Measure allocations, peak memory, latency, stack/container size
  and generated async future size. Include spill cases for inline storage and
  all repeated passes for an iterator design.
- Require differential leaf and Gateway tests for accepted heads and overlapping
  validation failures, plus independent IMPL plan/final review and project checks.
  Freeze any necessary leaf edit allowlist. No new body cache, full-response
  aggregation, tracing, payload logs, metrics or integration-fixture edits.

The current Gateway WAF baseline passes 51 focused tests. This verifies existing
behavior, not the performance or correctness of either proposed representation.
This task grants no implementation approval.
