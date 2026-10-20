# Evaluate request-body dispatch for inactive downstream modules

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Edgion baseline: `066c44e60`.
Inspected Pingora fork: `1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence

Edgion's `pg_init_downstream_modules` registers ResponseCompression and GrpcWeb.
Their implementations in the fork's `pingora-core/src/modules/http/` do not
override `HttpModule::request_body_filter`. Both inherit its async default,
which returns Ok without reading or modifying the body or event.

When the request relay reaches its body-hook stage,
`pingora-proxy/src/request_relay.rs` awaits
`downstream_modules_ctx.request_body_filter` before the application
`request_body_filter_action`. Earlier trailer termination or error can bypass
that stage. `HttpModuleCtx` iterates all registered boxed modules in order and
awaits each request-body hook.

The locked async-trait 0.1.92 transforms the trait method into a pinned boxed
dynamic Future and emits `Box::pin(async move { ... })`. This establishes a
source-level boxing candidate even for the two default hooks. It does not
establish a measured allocation count per event, optimized allocator behavior,
CPU cost or throughput benefit.

This differs from [outer module storage](context-01-downstream-module-storage.md),
which concerns construction and retained context size, and from
[Stream plugin dispatch](stream-01-synchronous-plugin-dispatch.md), which concerns
connection/session admission.

## Decision and boundaries

Keep the current uniform dispatch, or authorize a separately scoped fork
investigation of explicit per-stage participation or a synchronous completion
path. A participation contract must not silently stop calling custom modules;
default participation must remain safe for existing overrides. A fast result
representation must define evaluation, error and cancellation timing. Do not
hardcode a bypass based on the two modules Edgion currently registers.

Preserve filter order, Option<Bytes> mutation, error propagation, active-module
awaits, typed get/get_mut availability, and application-hook ordering. Preserve
Data, Complete and Abandoned distinctions, retry replay and terminal delivery.
No module may infer these facts from hidden session state. Do not change
compression availability, gRPC-Web activation, configuration snapshots, response
framing or module registration to avoid request-body dispatch.

External fork edits require explicit authorization and an exact reviewed scope.
Do not edit Cargo's dependency checkout or change the dependency revision as
incidental cleanup. This task grants no external mutation authority.

## Acceptance for later implementation

Measure the actual dispatch chain for both default modules and active custom
modules, including a genuinely pending hook. Separate first-use setup from
steady-state allocation calls/bytes and CPU. Exercise empty/nonempty bodies,
small and large chunks, early hook errors, body replacement/removal, normal
completion, abandonment, retries and cancellation. Verify filter and application
call order, no skipped active hooks, and unchanged terminal outcomes. Include
the revised branch/metadata cost and extension-contract complexity in the value
assessment; do not report source boxing alone as a measured performance gain.

Relevant original knowledge:
`skills/04-review/h2-grpc/regression-guards/abandoned-request-body-terminal-event.md`.
This source-only investigation changed no runtime code and ran no runtime tests
or benchmarks.
