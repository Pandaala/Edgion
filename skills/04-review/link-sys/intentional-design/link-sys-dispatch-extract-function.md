---
name: link-sys-dispatch-extract-function
description: Use when reviewing link_sys runtime/store.rs "dispatch_full_set / dispatch_partial_update have extractable functions / duplicate provider match arms / should unify the two dispatch functions" findings (F-013, F-008). The credential-readiness helper is already extracted; the deeper full_set/partial_update unification is a deliberately declined refactor.
knowledge_type: intentional-design
---

# LinkSys store.rs dispatch functions — Extract-Function / Dedup — DESIGN-TRADEOFF

## Superseded implementation rationale

The user-authorized unified runtime table supersedes the separate dispatch and
Webhook-bookkeeping rationale below. Full and partial source events now share
one registry mutation path, with one provider apply function; the extra Webhook
publication table and per-provider pending/deleting state are removed. The
re-evaluation triggers fired: publication semantics converged and a real
cross-provider stale-WebHook defect was confirmed. See
[provider-owned runtime](provider-owned-runtime-and-simple-removal.md).

The rest of this page preserves historical accepted tradeoffs and evidence. It
does not prohibit this authorized refactor or require restoring duplicate maps.

**Audit date**: 2026-05-25 (re-confirmed 2026-06-09; current dispatch re-evaluated 2026-09-17)

The historical F-013/F-008 proposal was to extract provider-specific dispatch
functions or collapse full-set and partial dispatch behind a generic
`LifecycleAction`. The generic client-swap helpers and their per-provider new
maps were removed when the persistent runner was introduced. They are historical
context, not current reasons to keep the functions separate.

The current `credential_readiness<M>(ls, auth_fn)` helper supplies the common
readiness gate to the persistent-provider `apply_*` paths. Redis, Etcd,
Elasticsearch, Kafka, and OTLP use per-key `PersistentRunner` ownership;
Webhook alone uses ordered prepare/commit and runtime bookkeeping. The two
dispatch functions remain separate because their current input and cleanup
semantics differ, not because they need distinct generic swap helpers.

## Core Rationale

**1. The two functions have genuinely different control flow, not just a swapped lifecycle helper**

`dispatch_full_set`: snapshots the Webhook runtime map up front, carries over the
old Arc for not-ready Webhook resources, accumulates a new Webhook bookkeeping
map, and commits the complete Webhook manager registration before replacing
that map. It first retains only the runner keys present for each persistent
provider, then posts desired state to Redis, Etcd, Elasticsearch, Kafka, and
OTLP runners while processing the resources.

`dispatch_partial_update`: deletes foreign persistent runners when a key changes
provider, applies each changed resource, commits Webhook registrations per item,
and then removes deleted keys from every provider. A not-ready Webhook update
keeps the existing live registration.

The full-set present-key retention and Webhook full commit differ from partial
kind-change deletion and Webhook per-item commits. A shared wrapper would need
to represent those branches explicitly. Keeping separate functions makes the
ordering visible, at the cost of repeated provider match arms.

**2. The remaining full-set map belongs to Webhook bookkeeping**

`new_webhook_map` holds prepared or preserved Webhook clients until the manager's
full commit succeeds. Persistent runner maps are long-lived and are not
`replace_all` targets. Extracting a provider arm would still need to preserve
the full-set retention phase and the Webhook commit boundary; there is no
current collection of generic client maps to eliminate.

**3. Readability-only, off the data-plane hot path**

Both functions run on LinkSys config sync / hot reload (ConfHandler), not on the
per-request data path. Repeated arms can drift, but no current defect follows
from their duplication alone. A broad collapse of the ordering and cleanup
paths has regression risk without a demonstrated functional benefit. Same
disposition class as
[plugin-runtime-god-match.md](../../plugin-runtime/intentional-design/plugin-runtime-god-match.md) and
[style-consolidation-thresholds.md](../../review-method/intentional-design/style-consolidation-thresholds.md).

## Fix Suggestions Not Accepted

- "Extract each provider arm to remove generic new maps" — those maps no longer exist. Review any proposed extraction against the current retention and commit ordering.
- "Unify both dispatch functions behind a `LifecycleAction` enum / `dispatch_one` / `for_each_provider` closure (F-008)" — the full-set and partial paths still perform different cleanup and Webhook commits; no current bug justifies encoding both in one dispatcher.
- "Adding another provider requires editing both dispatch functions and the remove loop" — this maintenance cost remains, but is not by itself evidence of a runtime defect. All five persistent providers use runners; Webhook has its own publication boundary.

## Re-evaluation Triggers

Re-open only if:

- A new provider introduces substantial repeated dispatch logic that cannot be factored without obscuring retention or publication ordering.
- Full-set and partial cleanup and Webhook publication semantics genuinely converge.
- A real bug is traced to a provider arm being added/changed in one function but not the other.

## Reference Cases

- link-sys F-013 (`tasks/link/link-sys-F013-store-rs-extract-function-dispatch.md`), link-sys F-008 (`tasks/link/link-sys-F008-dispatch-full-set-partial-update-code-duplication.md`), audited 2026-05-25.
- `edgion-gateway/src/link_sys/runtime/store.rs` — `credential_readiness`, `dispatch_full_set`, `dispatch_partial_update`.
- Sibling refactor-decline decisions: [plugin-runtime-god-match.md](../../plugin-runtime/intentional-design/plugin-runtime-god-match.md), [style-consolidation-thresholds.md](../../review-method/intentional-design/style-consolidation-thresholds.md), [size-and-similarity-heuristics.md](../../review-method/intentional-design/size-and-similarity-heuristics.md).

## Resolution references

- `link-sys-F013` — 2026-05-25 (⚠️ accepted tradeoff).
- `refresh-link-sys-documentation-set` — 2026-09-14 (documentation refreshed after the Kafka trigger fired; tradeoff retained).
- `ISSUE-008` — 2026-09-17 (current lifecycle guidance and obsolete generic-map rationale corrected; no runtime behavior changed).
