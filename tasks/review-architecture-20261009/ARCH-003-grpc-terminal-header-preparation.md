---
id: ARCH-003
status: candidate
kind: architecture-debt
priority: P2
primary_partition: GW-ADAPTER
analyst: /root/analyze_adapter
independent_verification: confirmed
---

# gRPC terminal-header preparation has three owners above the shared wire writer

## Current ownership and evidence

`runtime/server/error_response.rs::send_grpc_trailers_only_frame` already owns
native/gRPC-Web frame selection, canonical terminal status and message encoding.
The preparation immediately above it is repeated in:

- `plugins/runtime/session_adapter.rs:632`, `send_grpc_error_direct`.
- `plugins/runtime/session_adapter.rs:692`, `send_grpc_trailers_only`.
- `runtime/server/error_response.rs:142`, `send_grpc_status_trailers_only`.

Each builds fresh HTTP-200 headers, drains pending response-header operations,
reconciles request-trace headers, removes Content-Length/Transfer-Encoding,
reasserts application/grpc and calls the shared frame writer. The framework path
first applies ServerHeaderOpts; this default-header distinction is legitimate.

## Call chains and maintenance scenario

Request-stage ErrResponse/ErrGrpcError reaches the adapter exact-status writer;
a self-writing plugin's non-200 gRPC response reaches the HTTP-mapped adapter;
response-stage Grpc denial reaches the framework writer through pg_fail_to_proxy.
Request-body termination also reaches the adapter exact-status writer.

A terminal-envelope preparation change, such as cleanup ordering around pending
operations, requires maintaining three copies across two modules. Changing only
the frame writer cannot implement policy around queue application, and changing
only the framework writer leaves adapter entry paths unchanged. Three ordered
policy implementations are observed; future path divergence is a projected cost.
No current protocol failure, incident or speedup is asserted.

The terminal-status-field guard records a historical divergence now corrected
in the common frame writer. It explains the maintenance seam, not a new bug.

## Legitimate distinctions and bounded recommendation

Extract one private terminal-gRPC preparation operation beside the canonical
error writer, accepting the existing fresh header and context. Existing callers
invoke it after their current claims/defaults and before the frame writer.
The sequence uses identical types and has no stage-specific branches.

Preserve separate exact versus HTTP-mapped status entry points, adapter termination
state and delivery diagnostics, framework duplicate-claim no-op versus adapter
error, framework defaults before queued conditions, existing protocol codec and
response-stage replacement/abort ownership. Ordinary finite HTTP replies and
protocol pumps retain their separate owners.

Delegating the HTTP-mapped adapter to its exact-status method reduces three copies
to two but does not unify framework/adapter preparation. A universal sender would
need a wider explicit design; the private preparation extraction is sufficient.

## Benefit and risks

One implementation owns future ordered preparation changes without moving caller
lifecycle policy. The production scope is two files plus focused owning tests.
No new allocation, lock, cache, body representation, feature capability or mutable
context escape hatch is needed. File movement/comments without eliminating the
policy copies do not deliver the proposed benefit.

## Required invariants and future verification

Preserve HTTP 200 plus independent exact gRPC status, single final claim and status
persistence after failed delivery, queue order/conditional semantics/one drain,
framework defaults, trace reconciliation, protocol-owned framing/content type,
canonical status/message replacement, native trailers-only/gRPC-Web frames,
adapter body suppression and diagnostics, access-log ownership/redaction and
streaming/body invariants. No new metrics or hot-path tracing.

Future tests should cover all three paths with queued headers, repeated status
fields, absent messages, conditional operations, request IDs, duplicate claims
and failed delivery, respecting legitimate default-header differences. Existing
adapter reroute, response-stage exact reply, gRPC-Web canonical frame and absent
message tests were read, not run. Wire evidence and project gates belong to a
future implementation.

## Prior records and independent disposition

Original graded rules/unification registry, plugin/lifecycle/development guidance,
response-header mutation policy, terminal-status-field ownership, declined gRPC
sentinel refactor, response-stage denial and fork async boundaries were read.
This preserves the previously declined sentinel rewrite and protocol-pump boundaries.
Task searches found no existing owner of this preparation extraction. ARCH-001
owns feature capability growth and does not overlap. Independent review is pending.
