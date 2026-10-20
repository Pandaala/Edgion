# AI accounting snapshot reuse

Status: deferred observation-point and performance decision for the user.
Source baseline: `e6a71b83f`. No runtime or schema change.

## Current evidence

Terminal HTTP logging reaches `ai/accounting/finalize.rs` after AI processor
finalization. Attempt-runtime finalization settles its active attempt and awaits
pending settlements before request usage and quota finalization.

`ai/accounting/usage.rs::accounting_snapshot` first checks observation-off and
unavailable/incomplete-document state. On a successful reported-only path it
reads reported_usage once to establish presence, then again to select the value.
The ordinary path performs only the latter read. Each read copies four counters
under a temporary ArcSwap guard; it does not call load_full to request an owned
Arc. The guard retains the dependency's owned fallback when needed. The earlier
borrowed projection optimization deliberately preserved these separate reads.

One processor publishes cumulative component high-water marks as immutable
multi-field snapshots. The current implementation publishes Some values and does
not clear them, but a higher snapshot can appear between two reads. Concurrent
logging is explicitly supported. Fusing these reads selects a different possible
observation point; this alone is not proof of an existing race defect or proof
that the result must stay byte-identical under every interleaving.

The terminal usage and quota finalizers also call accounting_snapshot separately.
Usage finalization stores GenAiSummary as its once-only metric/summary sentinel.
Quota finalization's second snapshot only checks arithmetic-overflow classification:
its authoritative charge is the attempt runtime's charged total, or the summary's
checked input/output sum when no attempt dispatched. Reusing a summary snapshot
must not replace those independent charge authorities. Re-entry can skip summary
construction while still reaching quota finalization.

## Separate alternatives

- Keep the current reads and their existing observation points.
- Fuse presence and value reads within one accounting_snapshot call, only after
  establishing the allowed concurrent observation contract and measured value.
- Reuse terminal accounting evidence across phases only under a separately
  reviewed producer-finalization, ownership and cancellation contract. This is a
  broader change than local read fusion and could retain stale flags or overflow
  classification. No cache, new snapshot type or API is selected here.

A Copy value does not make its acquisition redundant. Nor does an immutable
published counter tuple make separately stored observation/lifecycle flags one
atomic aggregate. Do not add a request-wide cached result that bypasses producer
updates or changes missing reported-only usage into estimated billing.

## Acceptance for an approved implementation

Preserve the canonical usage owner, single-producer high-water publication,
complete four-field reads, optional cache-detail normalization and overflow
handling. Keep observation-off, unavailable, incomplete JSON and missing-provider
precedence. Native reported-only traffic must remain reported-only; stream and
single-document duplicate rules remain distinct.

Identify an explicit valid observation point for each consumer. Verify publication
between presence/value reads, terminal producer completion, sticky unknown state,
missing usage, late overflow, aborted streams and cancellation/re-entry. Preserve
summary once-only metrics, attempt metric totals, credential settlement, charged
request totals, quota failure diagnostics and finalization ordering. Do not hold a
snapshot guard or a new lock across asynchronous settlement. A cross-phase design
must prove that its retained evidence remains valid, including paths without a
new summary; terminal naming alone is not a producer-quiescence proof.

Measure current read costs and any added retained state or copying before
choosing a representation. Report within-call and cross-phase results separately.
No throughput or allocation saving is established by this source audit. Add no
request tracing, new metric family, response-body accumulation or second accounting
service for this optimization.

## Original knowledge and verification limits

- [Monotonic cumulative usage publication](../../skills/04-review/ai-routing/regression-guards/streaming-usage-is-a-monotonic-cumulative-snapshot.md)
- [Native Messages reported-only accounting](../../skills/04-review/ai-routing/regression-guards/native-messages-preservation-and-accounting.md)

Fresh verification passed 52 usage, 13 accounting-finalizer and 25 attempt-runtime
tests. Formatting and workspace all-target check/Clippy passed, with existing
unrelated warnings. These validate the current implementation, not a fused-read
or cached-evidence design. No concurrency stress experiment, live provider/Redis
exchange or throughput benchmark ran for this audit.
