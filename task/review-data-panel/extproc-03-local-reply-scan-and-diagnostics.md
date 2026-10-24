# ExtProc local-reply scan and diagnostic projection

Status: deferred for user decision. No runtime design selected.
Source baseline: `b581e4f40`.

## Evidence

The streamed request-body handler's ImmediateResponse branch first searches
set/remove header names with the canonical protected-response predicate to set
`has_log`. It then consumes the same mutation through `local_reply_ops`, which
checks protected names again while creating typed pending operations. The
preliminary `any` searches short-circuit at the first protected name; this is
not two guaranteed complete scans. It occurs on immediate replies, not ordinary
successful body chunks.

An aggregate protected-rejection flag returned by the canonical conversion could
remove the preliminary traversal without adding another policy implementation.
No helper API or result representation is selected. Existing conversion already
moves accepted names and preserves typed value bytes; those ownership choices
are separate from scan reuse.

There is also a diagnostic discrepancy. For a fresh handler with an immediate
reply containing only an allowed header name and invalid HTTP value bytes,
`decoded_value` skips the operation and records the static `invalid header value`
error. No protected name sets `has_log`, so `take_log` returns None and terminal
`drain_handler_logs` cannot publish that buffered error. A protected sibling
would set the flag and allow the buffered errors to drain. The existing streamed
test covers protected rejection delivery, not invalid-only diagnostic delivery.
This conclusion comes from the source chain, not a newly executed reproducer.

The diagnostic discrepancy is not an accepted intentional policy. The owning
opaque-header knowledge requires invalid bytes to be skipped with a static
PluginLog error, but recording that buffer alone does not establish delivery.

## Decision and boundaries

Choose whether to retain the scan pending evidence or investigate a single
canonical conversion result carrying its rejection summary. Separately scope
correction of invalid-only diagnostic delivery. A protected-only summary can
preserve the current flag semantics but leaves that known discrepancy unresolved;
a summary of every emitted diagnostic changes visible logging and must be
acknowledged and verified as a correction, not called behavior-preserving.

Preserve protected/pseudo-header policy, nonempty raw-value precedence, textual
fallback, keep_empty_value, all four append actions, unknown-action fallback,
set-before-remove order and fresh-response conditional application. Keep the
same static diagnostic text, ordering and bounded truncation. Never include raw
header values or processor secrets. Retain terminal-only handler log draining and
the attached owner's sole aggregate transaction summary; add no per-chunk log,
metric, tracing, new body store or transport exchange.

This does not select the [value ownership proposal](response-03-extproc-local-reply-value-ownership.md)
or [protobuf body representation](body-02-extproc-protobuf-body-storage.md).

## Acceptance for a selected implementation

- Freeze the API/diff allow-list and obtain independent IMPL plan/final review.
- Compare accepted operations and order for absent headers, valid-only headers,
  protected set/remove names, pseudo-headers, mixed case and all append actions.
- Explicitly test invalid-only values, invalid plus protected siblings, empty
  values with both keep flags, opaque valid bytes and repeated drain calls.
  State whether expected diagnostic delivery preserves or corrects baseline.
- Verify streamed immediate replies and request-stage queued replies share one
  conversion policy without duplicating aggregate terminal outcomes.
- Measure the preliminary predicate work against summary bookkeeping for small
  and larger mutations. No allocation, latency or throughput saving is measured.

Sources: `plugins/http/ext_proc/{streamed,mutation}.rs` and
`routes/http/proxy_http/pg_request_body_filter_action.rs` in `edgion-gateway/src/`;
the canonical predicate is in `edgion-resources/src/response_header_policy.rs`.
Relevant knowledge: [opaque header fidelity](../../skills/04-review/ext-proc/regression-guards/opaque-header-fidelity.md)
and [interactive verdict ownership](../../skills/04-review/ext-proc/regression-guards/ext-proc-interactive-continue-and-replace.md).

Fresh unchanged-source verification passed 147 ExtProc and 19 request-body-action
tests (166 total), formatting, workspace all-target check and Clippy with existing
warnings. No prototype, new diagnostic reproducer, live transport, full suite or
benchmark ran.
