# Evaluate shared original-case names for prepared fixed headers

Status: deferred optimization for user decision. No runtime change.
Inspected baseline: `4b4004162`.

## Evidence and decision

`edgion-gateway/src/plugins/runtime/prepared_header.rs` owns a String name and
cold-prepared, shared HeaderValue. Native HTTP/gRPC request and response header
modifiers and ResponseRewrite fixed add/set entries borrow that name into the
PluginSession typed writers. The real adapter copies it into a String for each
reached request write or bound response write. Unbound response writes copy the
name into PendingHeaderOp instead; normal application later clones the operation,
while local-reply drain consumes it. Preparing only the value does not remove
these name copies. AI `ai/runtime/dispatch.rs::apply_backend_request_headers`
also borrows the native backend modifier's same plan and calls typed request
writers directly; it is part of this consumer set, not a separate preparation.

The pinned Edgion Pingora revision `1aaa45cc70430bc782990bc20ddee3fef36d6b99`
converts String into original-case Bytes. Public Bytes is also an accepted input.
CaseHeaderName internally owns Bytes, but its module and root import are private;
it is not an externally nameable field type for the Gateway. A parsed HeaderName
is not an equivalent case-preserving substitute: standard names use Pingora's
title mapping and other names use their normalized spelling. Arbitrary borrowed
str does not implement the static-string conversion. Both insert_header_value
and append_header_value still parse a normalized HeaderName from original-case
bytes, even when the input already has shared ownership.

Choose whether to prepare a shared original-case owner for these fixed names or
retain String storage and per-write copies. A candidate could replace the runtime
String rather than automatically retaining both representations, but must provide
borrowed text for existing policy and bookkeeping consumers. Carrying the owner
through the PluginSession boundary and pending-response queue needs an explicit
representation design. A direct-bound-only proposal must account for its remaining
queued copies and avoid two competing policy implementations. A Pingora API change
is not established as necessary. No global cache, request-time lazy preparation,
schema field or conf_sync payload change is justified.

The tradeoff includes cold construction, first-clone promotion, shared-buffer
metadata/refcounts, short or never-used names, old runtime generations and live
requests/responses. It does not establish reduced normalized-name parsing, total
heap use, allocation count or throughput. Measure these before selecting a shape.

## Scope and acceptance for a later implementation

Start with the existing PreparedHeader consumers: native modifiers (including
AI's direct backend request application) and ResponseRewrite fixed add/set entries. Dynamic rename values, remove operations,
other string writers, ForwardAuth and persistence issuance are outside this
initial scope. Freeze an exact file allow-list after deciding the owner/API shape.

- Preserve original case in H1 serialization and case-map enabled/disabled paths,
  valid repeated values, Set-Cookie append behavior, insertion errors, typed opaque
  bytes and sensitivity. Do not bypass Pingora map ownership through raw writes.
- Preserve native first-equivalent configured-name deduplication and set/add/remove
  order. ResponseRewrite preserves every duplicate and status/rename/add/set/remove
  order, error logs, modified tracking and invalid-construction no-op behavior.
- Preserve AI's persistent rejects_invalid_config gate before any mutation,
  shared-plan parity with ordinary backend execution, and InvalidConfiguration
  errors from rejected writes.
- Keep request sealed-RPC rejection and deferred-credential invalidation after a
  successful write. Prepared names do not authorize bypassing live request policy.
- Keep canonical response protection and policy-before-value error precedence,
  typed entry guards, queued malformed-value behavior, independent queue admission
  and application guards, conditional replay ordering and local-reply framing.
  Normal application retains its queue; destructive drain consumes it.
- Keep canonical cold validation, immutable runtime publication and old-generation
  request isolation. Keep Pingora-specific representations in Gateway runtime,
  not resource schema or transport. Add no logging, metrics or new cache.
- Compare current/prepared cold, first-use and warm writes; request, bound response
  and queued/replayed/drained response paths; short/long standard/custom/mixed-case
  names and repeated values. Measure allocations and retained bytes across unused
  configurations, reload and overlapping requests. Separate removed payload copies
  from normalized parsing, refcounts and HeaderMap work.
- Obtain independent IMPL plan/final review, meaningful ownership/wire evidence,
  targeted tests and project checks. Existing integration fixtures remain frozen.

This extends fixed-value preparation, not the separately deferred
[borrowed/owned queue execution](response-01-pending-header-ownership.md).
The [ForwardAuth denial](forward-auth-01-prepared-denial-header-names.md) and
[session-persistence name](lb-04-prepared-session-header-name.md) tasks have
separate consumer policies and frequency; this task does not authorize them.
Their references to CaseHeaderName need the visibility qualification above when
those tasks are resumed; prefer a publicly usable representation, not that type.
Original policy and preparation knowledge is in
[response mutation policy](../../skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md).

Fresh baseline tests passed 65 native Gateway API filter, 10 ResponseRewrite and
100 real-adapter cases. These verify current behavior, not a proposed owner or
performance result. No candidate implementation, live integration, full workspace
test suite or benchmark ran.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated Clippy warnings.
The same-plan AI consumer was also verified: all 36 ai::runtime::dispatch::tests
passed, including ordinary/AI modifier parity and rejected-generation coverage.
Agent-doc validation and diff hygiene passed again after the knowledge correction.
