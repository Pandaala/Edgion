---
name: link-sys-dispatch-extract-function
description: Use when reviewing link_sys runtime/store.rs "dispatch_full_set / dispatch_partial_update have extractable functions / duplicate provider match arms / should unify the two dispatch functions" findings (F-013, F-008). The credential-readiness helper is already extracted; the deeper full_set/partial_update unification is a deliberately declined refactor.
knowledge_type: intentional-design
---

# LinkSys store.rs dispatch functions — Extract-Function / Dedup — DESIGN-TRADEOFF

**Audit date**: 2026-05-25 (re-confirmed 2026-06-09; re-evaluated 2026-09-17 after Kafka joined the persistent runner)

The one concrete, low-risk extraction these findings asked for — a shared
credential-readiness prelude helper — is **already implemented** as
`credential_readiness<M>(ls, auth_fn) -> (ready, tls_ok, auth)`
(`edgion-gateway/src/link_sys/runtime/store.rs`), called from Redis/Etcd/Elasticsearch/Kafka
`apply_*`. Webhook still uses `preserve_if_not_ready`. The former generic
lifecycle helpers (`register_for_full_set`, `swap_and_shutdown_olds`,
`spawn_and_swap_partial`, `carry_over_if_not_ready`, `spawn_shutdown_if_present`)
were removed when Kafka moved onto the persistent runner.

The remaining suggestions — per-provider `dispatch_full_set_<provider>`
functions (F-013 #2) and collapsing both `match &ls.spec.config` blocks into a
single generic `dispatch_one` + `LifecycleAction` enum (F-013 #3 / F-008) — are
**not accepted**. Fix suggestions of the form "extract one function per provider
to remove the eight `new_*_map` / `preserved_*_keys` locals" or "unify
`dispatch_full_set` and `dispatch_partial_update` behind a `LifecycleAction`
enum / `for_each_provider` closure" are not accepted.

## Core Rationale

**1. The two functions have genuinely different control flow, not just a swapped lifecycle helper**

`dispatch_full_set`: snapshots the Webhook runtime map up front, carries over the
*old snapshot's* Arc for not-ready Webhook resources, accumulates into a fresh
Webhook map, and after a **single** pass over `data` replaces the Webhook map
atomically. Redis/Etcd/Elasticsearch/Kafka post desired state to per-key
persistent runners in the same pass and `retain_*` keys that disappeared.

`dispatch_partial_update`: preserves in place against the **live** runtime,
swaps **per item**, and runs a separate uniform remove loop.

Carry-over-from-snapshot vs preserve-in-place, swap-all-at-end vs swap-per-item,
and the SHUTDOWN_ON_REPLACE-gated bulk eviction are real semantic differences.
A `LifecycleAction` enum would have to re-encode both flows behind branches,
re-introducing the divergence it claims to remove while adding indirection — and
the swap/carry-over semantics are exactly the kind of thing a mechanical
collapse silently regresses.

**2. The provider maps are inherent to "single pass + per-store atomic replacement at the end"**

`new_*_map` / `preserved_*_keys` exist because one loop interleaves four generic
providers plus Webhook preparation, and publication happens *after* the loop.
Per-provider extraction can't remove them without either iterating `data` repeatedly (changes pass count
and operation ordering) or building a generic accumulator struct (more machinery
than the locals it replaces). What's left is a form-only refactor.

**3. Readability-only, off the data-plane hot path**

Both functions run on LinkSys config sync / hot reload (ConfHandler), not on the
per-request data path. There is no correctness risk — the repeated arms compile
identically. The cost (restructuring two working, tested functions that touch
config sync) and regression risk are out of proportion to a marginal
readability gain. Same disposition class as
[plugin-runtime-god-match.md](../../plugin-runtime/intentional-design/plugin-runtime-god-match.md) and
[style-consolidation-thresholds.md](../../review-method/intentional-design/style-consolidation-thresholds.md).

## Fix Suggestions Not Accepted

- "Extract `dispatch_full_set_<provider>` per provider to drop the provider maps" — the locals are structural to single-pass-then-publish; extraction would force repeated passes over `data` or an accumulator struct, neither of which is a net improvement.
- "Unify both dispatch functions behind a `LifecycleAction` enum / `dispatch_one` / `for_each_provider` closure (F-008)" — the two functions' carry-over/swap semantics genuinely differ; collapsing trades real regression risk for marginal readability.
- "Adding another generic provider requires editing both dispatch functions and the remove loop" — known trade-off, not a defect. Redis/Etcd/Elasticsearch/Kafka use persistent runners; Webhook and OTLP intentionally use separate publication models.

## Re-evaluation Triggers

Re-open only if:

- Another provider is implemented on the **generic** lifecycle and materially increases the repeated readiness/build/swap/remove branches. Kafka and OTLP use the persistent runner; Webhook still needs ordered prepare/commit publication, HttpDns is resolver-consumed, and CredentialSource is Controller-only. Those variants are not evidence that the remaining Webhook branch should be collapsed.
- The `full_set` carry-over-from-snapshot and `partial_update` preserve-in-place semantics genuinely converge, so a single dispatch path no longer has to encode two flows.
- A real bug is traced to a provider arm being added/changed in one function but not the other.

## Reference Cases

- link-sys F-013 (`tasks/link/link-sys-F013-store-rs-extract-function-dispatch.md`), link-sys F-008 (`tasks/link/link-sys-F008-dispatch-full-set-partial-update-code-duplication.md`), audited 2026-05-25.
- `edgion-gateway/src/link_sys/runtime/store.rs` — `credential_readiness`, `dispatch_full_set`, `dispatch_partial_update`.
- Sibling refactor-decline decisions: [plugin-runtime-god-match.md](../../plugin-runtime/intentional-design/plugin-runtime-god-match.md), [style-consolidation-thresholds.md](../../review-method/intentional-design/style-consolidation-thresholds.md), [size-and-similarity-heuristics.md](../../review-method/intentional-design/size-and-similarity-heuristics.md).

## Resolution references

- `link-sys-F013` — 2026-05-25 (⚠️ accepted tradeoff).
- `refresh-link-sys-documentation-set` — 2026-09-14 (documentation refreshed after the Kafka trigger fired; tradeoff retained).
