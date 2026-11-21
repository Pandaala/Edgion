---
name: filter-iteration-control-sync-phase-tradeoff
description: Review the boundary between synchronous Edgion plugin traits, asynchronous Pingora hooks, and fork response lifecycle capabilities.
knowledge_type: compatibility-constraints
---

# Response plugin iteration and fork async capability boundaries

## Conclusion and scope

The local Pingora path patch adopts upstream `018abe0` for the downstream
body callback (upstream-main issue 04). Edgion inherits its async default.
Its synchronous read-only body plugins and asynchronous streaming processors
run in the upstream stage and are skipped on ordinary cache hits.

At the previously inspected Pingora revision
`edgion_v0.9.1@e5f4287be713d01ffd444544251bf75c9d00062a`,
`ProxyHttp::upstream_response_filter`, the upstream body hook, and the upstream
trailer hook are asynchronous, while the downstream `response_body_filter`
remains synchronous. This was re-verified by source inspection at that
revision on 2026-11-06 after the fork was rebased onto upstream 0.9.1; the
original inspection used the then-locked
`0b0d8bba9609655bfd7db0fe0add6818bc0ee937` (historical). Edgion's `UpstreamResponseFilter` and read-only
`UpstreamResponseBodyFilter` plugin traits remain synchronous. Their signatures
are a product phase boundary, not evidence that Pingora cannot await response
work. Writable body processors and trailer companions already run asynchronously
through the request-owned `ResponseProcessorDriver`.

An asynchronous callback alone does not replace typed terminal events, bounded
extra output, processor cancellation ownership, or a precommit response-head
decision. Preserve those contracts when evaluating a smaller fork delta.
This is a constraint of the inspected APIs, not a permanent ban on redesign or
a requirement to preserve legacy aliases. Request-only BodyStorage and streaming
responses remain governed by [the canonical design rules](../../../00-design-rules.md).

## Rationale and evidence

Edgion consumer inspection originally used HEAD
`c6848316bc4c7a3ffe0a3eb2fa3d60b2f5e50076` and the then-locked `0b0d8bb`; the
files below still exist, and the adapter still overrides the upstream
header/body/body-event/trailer hooks as `async fn` and has no production
`response_body_filter` override, as re-checked on 2026-11-06:

- [Proxy adapter](../../../../edgion-gateway/src/routes/http/proxy_http/mod.rs):
  asynchronous upstream header/body/trailer callbacks and response-head hooks.
- [Body adapter](../../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs):
  maps `Data`, `TerminalBeforeTrailers`, and `TerminalWithoutTrailers` to
  processor callbacks without collapsing the trailer lifecycle.
- [Processor driver](../../../../edgion-gateway/src/response/driver.rs):
  body and trailer groups retain separate cancellation latches; dropping an
  incomplete execution lease releases the active group's inflight ownership.
- [Trailer adapter](../../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_trailer_filter.rs):
  distinguishes real trailers from their absence, dispatches serially, and
  returns a non-retryable error on fail-close.
- [Synchronous header trait](../../../../edgion-gateway/src/plugins/runtime/traits/upstream_response_filter.rs)
  and [read-only body trait](../../../../edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs):
  retain their own synchronous phase contracts. The
  [DSL plugin](../../../../edgion-gateway/src/plugins/http/dsl/plugin.rs)
  no longer uses the synchronous header trait; its response script runs in
  the async `UpstreamResponse` stage without a `block_in_place` bridge.

The fork's `proxy_trait.rs`, `response_pipeline.rs`, `response_body_sink.rs`,
`response_terminal.rs`, and `response_head_barrier.rs` own the generic
contracts. Its feature documentation under `edgion-changes/features/` and
`edgion-changes/architecture/body-relay.md` is the detailed authority. All of
these paths exist at `e5f4287`. In the rebased fork the precommit decision is
the response-head Hold (`ResponseHeadCommitPlan::{Immediate, Hold}` with
sink-driven Release/Replace, the synchronous `response_head_hold_boundary`
mapper that defaults to a non-retryable Fail, and a single absolute deadline),
and `response_head_may_hold` disables cache lookup/admission before the cache
key is generated.

| Operation | Inspected upstream async commits | Required fork/consumer behavior |
| --- | --- | --- |
| Await body/trailer callbacks | Supported | Already supported; await preserves borrowed session/context ownership |
| Body output | Mutable current chunk | Sink extras have independent 1 MiB and 2048 nonempty-chunk budgets per batch; current-chunk growth is separately processor-bounded |
| Response termination | No typed terminal/sink contract added | One clean terminal boundary, before real trailers; failure is not clean EOS |
| Callback cancellation | No Edgion processor ownership contract added | Body/trailer leases close only the cancelled group |
| Precommit response decision | No Hold contract added | Bounded Release/Replace/Fail before writer handoff, with an absolute deadline |
| Downstream body callback | Changed to async | Local path patch awaits the inherited callback on live and cache-hit responses |

Upstream commits
[`018abe0`](https://github.com/cloudflare/pingora/commit/018abe093903ee926dd3a75a8c966698101ce886)
and [`6cc56b6`](https://github.com/cloudflare/pingora/commit/6cc56b6c10b086359e9c97f0bfe5a0d7d41cec1f)
add async signatures, awaits, and context bounds; they do not add the richer
contracts in this table. This comparison is pinned to those commits, not an
assertion about all future upstream releases. Neither commit is in the upstream
0.9.1 release (`f86ffda`) that the current fork is based on; at `f86ffda` the
upstream body, trailer, and downstream body hooks are still synchronous, so the
async upstream hooks at `e5f4287` are fork changes.

H1, H2, and custom pumps perform upstream compression before the shared response
pipeline. Upstream processor output precedes cache admission and downstream-only
transformation. Cache hits skip upstream processors. Hold bypasses cache before
lookup/fill and rejects unsupported cache/custom combinations. Keep
[protocol-specific pumps separate](../../h2-grpc/intentional-design/pingora-protocol-pumps-remain-separate.md);
matching callback signatures do not establish matching reuse or cancellation semantics.

## Alternatives and limits

On 2026-10-05 the user explicitly requested closure of P090-009 and postponed
convergence work. Retain the current implementation for now. The accepted cost
is continued maintenance of the fork delta; no measured production regression
or safe adapter-removal benefit was established. The proposed removal list is
empty under this inspection.

This closes the active investigation by user decision; it does not claim that
upstream convergence was implemented, all acceptance scenarios were exercised,
or a defect was fixed/disproved. Future investigation requires a new scoped
task rather than treating this record as implementation approval.

The historical `filter-02` decision from 2026-05-29 accepted synchronous plugin
iteration and the bounded DSL bridge. Its original assertion that the underlying
Pingora hooks were necessarily synchronous is superseded by current code, and
the DSL bridge itself was removed by dsl-08; see
[the DSL async response-stage guard](../../concurrency/regression-guards/dsl-response-stage-async-no-block-in-place.md).
The historical response-replacement gap is superseded by
[response-stage denial handling](../../http-body/regression-guards/response-stage-denial-must-terminate.md).

## Verification and re-evaluation

This closure changes knowledge and task navigation only. Current evidence is
source/call-chain inspection and comparison with the two upstream commits.
No convergence benchmark or protocol integration acceptance was run.
Fresh closure verification on 2026-10-05 passed `cargo fmt --all`,
`cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets`,
and the agent-doc, SSA-force, metrics-inventory, Pingora-source, Gateway API
transform-contract, and unit-test-layout guards. Independent read-only review
returned LGTM. No full test suite was run for this closure.

Revisit when upstream supplies a specific missing lifecycle contract, a named
consumer needs asynchronous downstream filtering, or measured maintenance or
runtime cost justifies an exact adapter-removal plan. Before implementation,
independently review that plan and benchmark default no-processor dispatch.
Exercise fragmented SSE, slow callbacks, body/trailer cancellation, errors,
local replacement, 1xx/101/HEAD, trailers, cache paths, and clean/aborted
completion across H1/H2/custom capability boundaries. Preserve request upload
abandonment, retry eligibility, H1 reuse, H2 stream isolation, and logging bounds.

## Resolution references

- `filter-02-no-iteration-control` / `filter-02` — historical accepted tradeoff,
  2026-05-29; current dependency premise corrected during P090-009 inspection.
- `P090-009` — closed at user request on 2026-10-05; convergence postponed,
  production implementation unchanged.
