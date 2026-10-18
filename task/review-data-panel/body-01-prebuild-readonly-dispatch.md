# Evaluate a prebuilt read-only response-body runtime chain

Status: deferred for user decision. Priority: P2. Type: performance investigation.
Inspected baseline: `7d891d438`.

## Evidence and potential benefit

`pg_upstream_response_body_filter.rs::run_read_only_body_plugins` selects and
clones the route runtime, selected-backend runtime, Gateway runtime slice and
global runtime slice for every response-body invocation. Its sibling
`check_read_only_ip_dependencies` repeats the selection on a pre-trailer event
with no flushed data. Runtime handles permit mutable context borrowing; simply
turning these local clones into references is not sufficient.

An immutable request/attempt-owned dispatch list might amortize selection and
handle traffic across chunks. Evaluate building it at the successful final
response-header boundary, after upstream retries have selected the surviving
backend. That freeze point is a proposal, not an established invariant. Extra
list allocation and retained references may cost more on short responses.
There is no benchmark establishing a net CPU, throughput or latency benefit.

## Why this needs a decision and design

- `PluginRuntime::run_upstream_response_body_plugins` checks live IP
  dependencies **before** returning for an empty body-filter vector. Empty
  filters do not prove no work. Freeze identities only, never availability,
  condition results or captured IP-list generations across body callbacks.
- Preserve gRPC-over-HTTP preference separately for route and backend, and
  route -> backend -> Gateway -> global order, including duplicate runtimes.
  Preserve maximum-delay aggregation and the existing denial short-circuit.
- Prove list construction/invalidation across retries, cache hits, local
  replies, HTTPRoute/GRPCRoute, selected backend changes and captured plugin
  publication lifetime. Do not publish a partially initialized list.
- Preserve all typed events: Data with/without EOS, TerminalWithoutTrailers
  and TerminalBeforeTrailers. A pre-trailer event with no bytes still checks
  dependencies. Driver cleanup, queued-output discard and trailer preemption
  remain mandatory on denial.
- Keep cancellation leases and downstream-close monitoring intact. No
  request-context size growth, per-chunk logs, metrics or new cache may be
  introduced merely to measure this candidate.

## Acceptance evidence

Compare current and proposed dispatch with representative short and long
responses, no plugins, IP-only dependencies, multiple scopes, filters, retries
and hot reload. Measure per-request storage, allocations/Arc traffic, CPU and
tail latency. Include old/new publication isolation and IP revocation during a
stream, even when body-filter lists are empty. A synthetic idle stream alone is
insufficient evidence for broad adoption.

This is an internal ownership proposal, separate from the previously declined
public Pingora no-work hook. Respect the profiling requirement in
`skills/04-review/cpu-memory/intentional-design/async-response-body-hook-profile-before-gate.md`
and the cancellation contract in
`skills/04-review/plugin-runtime/regression-guards/response-callback-downstream-cancellation.md`.
