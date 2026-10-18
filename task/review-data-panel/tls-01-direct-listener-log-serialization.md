# Direct serialization of TLS listener log projections

Status: deferred for user decision. No production change applied.

## Current cost

Both `EdgionTlsTcpProxy::listener_log_entry` in
`edgion-gateway/src/routes/tls/proxy.rs` and
`EdgionTlsPassthroughProxy::listener_log_entry` in
`edgion-gateway/src/routes/tls/passthrough_proxy.rs` build a
`serde_json::Value` tree and then call `to_string()` before sending to the
listener logger. The projection allocates masked peer strings and materializes
nested upstream records, strings and log entries in the intermediate tree.
The final owned JSON string is still required by the asynchronous logger.
No throughput or whole-connection allocation measurement was performed.

A shared borrowed `Serialize` projection could remove the intermediate tree.
It must remain distinct from the global TLS context projection: listener logs
have their own fields and null/omission behavior.

## Why this is not an automatic change

The current Value-based output orders object keys through JSON maps. Direct
struct serialization emits declaration order, including nested upstream and
matched-resource records. Sorting only the outer fields does not preserve the
whole line. No normative TLS key-order requirement was found; the concern is
an observable byte-output difference, not an established correctness defect.
Semantic Value equality tests alone erase that difference and cannot prove
byte-for-byte preservation.

The `json!` construction also has a serialization-error panic boundary. A direct
serializer must explicitly retain or change the failure policy instead of
silently borrowing another logger's drop/fallback behavior. Current concrete
fields appear JSON-infallible, so this is a maintenance-contract question,
not a demonstrated runtime failure.

## Decision and bounded follow-up

Decide whether semantic JSON equivalence is sufficient for these listener logs,
or exact byte ordering must be retained. Specify serialization failure behavior.
If approved, implement one shared projection for the two TLS listener schemas;
do not redesign the logger channel, add a buffer pool or defer serialization
past the context lifetime.

Acceptance must preserve:

- All existing fields and nested structures, including null versus omission.
  Termination includes `tls_id` and no `mode`; passthrough includes
  `mode: passthrough` and no `tls_id`.
- Canonical direct/remote peer masking and nested backend-address masking in
  Off, Replace and Wrap modes, including custom markers.
- Existing plaintext SNI and already-rendered error/retry diagnostics. Do not
  blanket-mask diagnostics or apply masking twice.
- JSON escaping, original context immutability, the logging enable gate and both
  existing sinks. Only the final owned JSON crosses the asynchronous send boundary.
- The chosen ordering and serialization-failure contracts, tested explicitly.

Compare complete old/new serialization allocations, rather than measuring only
zero-allocation view construction. Run the existing isolated sensitive-mode
checks and exercise populated and absent fields, nested records and escaped text.

## Evidence boundaries

The original privacy contract is
`skills/04-review/tls/regression-guards/tls-log-address-redaction.md`.
The required owned output boundary is consistent with
`skills/04-review/cpu-memory/review-evidence/access-log-json-serialization-per-request.md`;
this proposal removes an intermediate tree rather than that final string.
Independent plan review confirmed the allocation opportunity and recommended
this deferral. Fresh existing tests passed: TLS 85, TCP 27 and shared relay 21.
These tests verify current behavior, not a proposed direct serializer or fresh
real-network integration/throughput results.
