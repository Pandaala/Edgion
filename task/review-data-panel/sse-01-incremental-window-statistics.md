# Incremental semantic window statistics

Status: deferred for user decision. No runtime change.
Source baseline: `b0b20e3c1`.

## Evidence and decision

GuardrailResponseProcessor::process_batch calls RawWindow::stats after each
successful record admission, on failed admission before resolving the old window,
and at EOS. stats calls scan(0), which sizes and admits token scratch, copies
fragment ranges into that scratch, validates and decodes each fragment, and sums
decoded lengths across the current buckets. Multiple small records below the
decision threshold therefore revisit growing prefixes within one bounded window.
The raw rollback counter optimization does not remove this semantic work.

Compare the existing on-demand scan with generation-owned incremental semantic
summaries in canonical RawWindow. Such summaries could reduce repeated decoding,
but require a representation and lifecycle decision: per-fragment validation
results, per-bucket totals, scratch requirements and failure ordering must remain
consistent with the current observation boundary. Do not add a plugin-local
semantic cache, a second decoder or retained decoded response text.

This is not just a pure length query. stats records scan errors in
semantic_degraded and semantic_failure. token_scratch admits the maximum token
length against current resident capacity before validating any token. A later
raw-only append can make that admission fail even when every previously scanned
fragment remains unchanged. Reusing a prior successful total cannot skip this
check. Buckets are scanned in key order; validating at append time can change
which malformed fragment or competing limit fails first.

No prototype or benchmark establishes the net benefit. Additional state has a
residency and maintenance cost; retaining scratch also changes allocation
lifetimes. The window already has independent hard bounds, so this is a
repeated-work opportunity, not evidence of unbounded response aggregation.

## Boundaries and acceptance

- Preserve current validation timing, error categories and precedence by default.
  Incremental bookkeeping need not change semantics, but equivalence must be
  demonstrated. Any proposal to relax existing rejection boundaries requires a
  separate explicit decision.
- Keep scratch and metadata admission against the current window, with checked
  arithmetic and capacity charged before allocation. Account for new state and
  all simultaneously live storage; logical byte counts are not heap bounds.
- Preserve failed try_retain atomicity and the unchanged returned record.
  Successful admission, retry, bucket insertion, raw-only records, unavailable
  semantics and coalescing must not leave stale or partially published summaries.
- Reset generation-local facts at the canonical release/replace/discard and
  start_next/finish boundaries. Preserve emitted-text overlap, context-only
  buckets, terminal cleanup and semantic-degradation behavior. A cached error
  must not silently replace the existing re-evaluation behavior of stats.
- Preserve Guardrail threshold decisions, outbound-call count and order,
  rollback reservation, failure policy, held-head behavior, summaries and EOS.
  Keep main-response streaming and aggregate observability; no body-phase logs,
  response BodyStorage or general response cache.

After user selection, use IMPL with an exact implementation allow-list. Compare
against the existing scan across UTF-8 and escaped JSON tokens, malformed inputs,
multiple buckets inserted in different orders, fragmented/coalesced storage,
raw-only appends after successful scans, repeated stats calls, competing failure
conditions, exact and exceeded capacity limits, failed retain/retry, and every
generation transition. Verify returned stats, errors and subsequent lifecycle
behavior before measuring allocations, retained capacity and decoding work.
Include small windows and pass-heavy traffic as well as many small fragments.
Do not extrapolate a statistics improvement to all window operations.

## Sources and scope

- Gateway ai/guards/semantic/response.rs: process_batch.
- Gateway sse/window.rs: stats, scan, token_scratch, visit_bucket, try_retain,
  complete_generation and clear_generation.
- skills/01-architecture/02-gateway/00-overview.md: canonical SSE substrate.
- skills/04-review/sse/regression-guards/window-capacity-admission.md.
- skills/04-review/sse/regression-guards/window-emitted-context.md.

This is distinct from [Guardrail snapshot encoding](guardrail-02-decision-snapshot-encoding.md),
which addresses owned snapshot text and request serialization. No runtime test
or benchmark was run for this documentation-only proposal.
