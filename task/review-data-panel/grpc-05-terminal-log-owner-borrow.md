# Evaluate borrowed transcoder ownership during terminal logging

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `defbd3300`.

## Evidence and potential benefit

`routes/http/proxy_http/pg_logging.rs::logging` awaits ordinary processor
finalization, then clones `ctx.grpc_transcoding`, an optional Arc. It records
interruption when a proxy error exists, calls `GrpcTranscodingPlan::finalize_log`
and clears retained output. This last sequence is synchronous.

`plugins/http/grpc_json_transcoder/transport.rs::finalize_log` takes the entire
mutable context, although it only writes RequestInfo gRPC status and stage logs.
The clone lets the owner be used while borrowing the context mutably. Simply
removing it does not resolve the conflicting borrows.

Narrowing the finalizer inputs and exposing disjoint context borrows could remove
one Arc increment/decrement per selected transcoder terminal branch. There is no
established heap-allocation saving or throughput result. This is a value decision:
the benefit is small and requires an interface change across context, transport
and logging, unlike a local redundant clone deletion.

## Candidate and constraints

Investigate an accessor that borrows the plan while separately lending mutable
RequestInfo and stage logs. Existing `stage_logs_and_attached_ext_proc` demonstrates
the disjoint-field pattern, but does not itself provide the required fields.
Do not expose unrelated context state or add per-request storage for this change.

Preserve the single outcome-lock critical section, Pending-to-Interrupted
normalization before the logged gate, once-only status/log publication and first
matching plugin entry. Within each stage, filters precede resource logs. Preserve
the current failure precedence, validated status and fixed diagnostic text.
Keep ordinary driver finalization before this sequence and output/reservation
cleanup after log enrichment. Cleanup takes ready output, terminal bytes and
header template before releasing the plan's reservation.

A take-and-restore shortcut temporarily removes the context owner. Unwinding
before restoration would leave it absent, whereas the current context retains
its handle. This is an ownership difference to assess, not evidence of a reachable
normal-path failure. Avoid adopting the shortcut solely because it compiles.
Borrowing disjoint fields can retain owner presence without this temporary state.

The canonical outcome and memory boundaries remain in
`skills/04-review/h2-grpc/regression-guards/grpc-json-transcoder-admission-and-outcomes.md`.
No new tracing, metrics, payload diagnostics or response buffering is needed.

## Acceptance evidence

Review actual interface complexity against the small reference-count saving.
Verify owner identity/lifetime, ordinary cleanup ordering, error and success
outcomes, absent/present validated status, repeated finalization, missing and
multiple matching plugin logs, and first-match ordering. Preserve the existing
terminal outcome matrix and cancellation coverage. Revalidate every caller after
narrowing the finalizer signature; baseline tests do not prove a future rewrite.

Fresh baseline verification passed 27 transcoder and nine logging tests, plus
formatting check, workspace check/Clippy and agent-doc validation. No candidate
implementation, live transport run or performance measurement was performed.
