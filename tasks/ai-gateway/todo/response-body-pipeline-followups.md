# Response-body pipeline — follow-ups deferred by the final review

| Key | Value |
|-----|-------|
| Created | 2026-08-05 |
| Status | todo — none of these block the feature; all were triaged as follow-up by the final whole-branch review |
| Source | `docs/superpowers/specs/2026-08-04-response-body-pipeline-design.md` + its execution ledger |
| Where | Items 1-3 are in the Pingora fork (`Pandaala/pingora`, branch `edgion`); item 4 is in `edgion-gateway` |

These are the residual items from the response-body pipeline work (async
upstream response-body hook, bounded 1→N emit, clean mid-stream terminate, and
the AI quota termination built on top). Everything blocking was fixed before
merge; these are the ones that were deliberately left.

## 1. `track_max_cache_size` does not count emitted chunks

`proxy_h1.rs` and `proxy_h2.rs` feed `track_body_bytes_for_max_file_size` only
the original task's payload, not chunks a filter emitted through the sink. Under
`CachePhase::Disabled(PredictedResponseTooLarge)` an emitting filter can
therefore under-count and let `response_became_cacheable()` fire for a response
that is actually over the limit.

Inert for Edgion today — Edgion never calls `session.cache.enable`, so no
Edgion path reaches it. Real for any future fork consumer that combines caching
with an emitting filter.

## 2. Cache-side consumption of `LeadingTask::Drop` is untested

`migrate_end_of_stream` (the shared decision function) and the delivery-side
consumption both have unit coverage. The cache-side consumption in
`cache_task_and_emitted_chunks` does not, because it needs a real `Session`.
It is a four-arm match over an already-tested decision, so inspection was
accepted in its place.

## 3. The fork has no H2 client test for terminate

`pingora-proxy/tests/test_upstream_response_body_sink.rs` exercises H1 only. The
h2 pump's terminate handling was verified by structural symmetry with h1 — the
re-review checked the specific properties the in-batch `break` depends on, and
Edgion's own `quota_termination_h2_framing` integration case covers H2
end-to-end from the consumer side. A fork-local H2 test would still be better;
`tests/seam/`'s Down×Up matrix is the right home.

Related, from the same review: the batch-race regression test
(`terminate_mid_batch_drops_only_the_leaked_tasks`) is a **probabilistic**
discriminator — it retries 20×, but a scheduler that always hands one task per
batch would silently reduce it to a duplicate of the cross-batch test. Closing
that properly needs a `#[cfg(test)]` hook exposing batch composition at the
`now_or_never()` drain. Flag if CI scheduling ever diverges qualitatively from
the development machine's measured multi-task-batch hit rate.

## 4. A deliberate gateway cut settles as an upstream-style incomplete response

An over-quota terminate reaches settlement with `eos == false`, which yields
`GenAiAttemptFailure::IncompleteResponse` and `SettlementOperation::Retain` — the
credential reservation is held at full value until its TTL instead of being
reconciled to actual usage.

This is conservative (it never under-charges) and an operator can distinguish
the cases via `quota_status: "over_limit"` plus the `999_12`
(`AiQuotaOverLimitTerminated`) status. But the reservation ledger itself cannot
tell "the gateway cut this deliberately" from "the upstream hung up", which is
worth fixing if reservation accuracy under quota cuts ever matters.

## 5. Note for whoever adds a non-streaming cache backend

The fail-closed guard on terminate-under-cache-readback keys on
`serve_from_cache.is_on()`, whose precise meaning is **"a streaming readback
consumer exists"**, not "a cache admission exists". `enable_miss()` only fires
when `session.cache.miss_body_reader().is_some()`, i.e. for a streaming
partial-write backend. On a non-streaming backend a miss admission is live while
`serve_from_cache` stays `Off`, so terminate takes the normal live-write path and
`release_cache_on_terminate` drops the uncommitted miss handler — correct today,
but the reasoning depends on that backend property.
