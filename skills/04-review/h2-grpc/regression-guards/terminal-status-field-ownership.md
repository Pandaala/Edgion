---
name: terminal-status-field-ownership
description: Review shared gRPC error writers and queued grpc-status or grpc-message fields across native gRPC and gRPC-Web.
knowledge_type: regression-guards
---

# Terminal gRPC status fields have one writer

## Conclusion and scope

`runtime/server/error_response.rs::send_grpc_trailers_only_frame` removes every
incoming `grpc-status` and `grpc-message` field before constructing the selected
terminal shape. Native gRPC puts the canonical values in trailers-only HEADERS;
gRPC-Web puts them only in its `0x80` trailer body frame. An absent canonical
message must remain absent, not inherit a plugin-queued value.

This applies to the direct Session status writer and both PluginSessionAdapter
writers. Ordinary queued headers remain intact. Framing and Content-Type are
still repaired by the existing callers, while status-field ownership is enforced
once at the shared terminal dispatch point.

## Rationale and evidence

[Design rule D](../../../00-design-rules.md#d-cross-cutting-unification--one-canonical-implementation-per-mechanism)
requires a single cross-cutting implementation. The canonical Session writer
already stripped these fields, but the adapter writers did not. The pinned
Pingora GrpcWebBridge rewrites Content-Type/framing without removing status
fields, so adapter responses could contain a queued initial status such as zero
and a contradictory terminal frame containing INTERNAL (13).

Changing DSL `terminate()` from bare fallback to explicit `ErrResponse` exposed
that difference in the request-stage adapter path. The correction moves the
existing cleanup into the shared writer rather than duplicating it across
callers or changing DSL semantics again.

## Alternatives and limits

Native gRPC with a supplied canonical message already overwrote the old values,
but that did not protect gRPC-Web or native replies with `message: None`.
The exact effect of contradictory status fields on every external client is
not established; the conflicting wire values themselves are the defect.
Message encoding remains governed by [the existing encoding guard](grpc-message-encoding.md).

## Verification and re-evaluation

The new `dsl_terminate_grpc_web_has_only_canonical_trailer_status` test uses real
DSL source/bytecode and an active in-memory GrpcWebBridge. It checks absence of
initial status fields, ordinary header retention, and the exact single trailer
frame after decoding HTTP/1 chunking. The
`grpc_terminal_frame_without_message_removes_stale_status_fields` test covers
duplicate input statuses and no canonical message. Both failed against the
uncorrected writer. Post-fix results and the independent review are recorded in
[the DSL correction guard](../../dsl/regression-guards/terminate-explicit-local-response.md).

Re-evaluate if a caller bypasses the terminal writer, the bridge changes its
header/trailer ownership, or local-response queue semantics change. In-memory
codec coverage is not full proxy, cross-process, or hot-reload coverage.

## Resolution references

- `reconcile-dsl-terminate-contract` — Astra review found this regression while
  reviewing the explicit DSL response; the user authorized the shared-writer
  correction on 2026-09-13.
