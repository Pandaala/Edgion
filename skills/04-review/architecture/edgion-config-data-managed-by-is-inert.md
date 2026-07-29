---
name: edgion-config-data-managed-by-is-inert
description: Use when reviewing EdgionConfigData ownership findings; covers why no `edgion.io/managed-by` arbitration exists between EdgionCenter and an operator, and why building one is not accepted.
status: wont-fix
finding_id: new_work-15
closed: 2026-07-28
---

# `edgion.io/managed-by` Is Inert on EdgionConfigData — WONT-FIX (no ownership mechanism)

## Conclusion

EdgionConfigData is the Center-controlled data overlay with a **single intended writer**, and it
is deliberately not an ops-repo artifact. There is consequently no ownership to arbitrate, no
`edgion.io/managed-by` reader, and none planned. The label is inert on this kind: nothing in the
workspace reads it, and the `managed_by` constants in `edgion-resources/src/constants/labels.rs`
have no callers at all.

Fix suggestions of the form "honor `edgion.io/managed-by` on ingest and reject a non-owning
writer", "implement the `center` / `controller` values", or "add an ownership-epoch field so a
Center write can win against a GitOps reconciler" are **not accepted**.

## Core Rationale

**1. The premise of the finding — git and Center contending for the same document — is not the design.**

The base/overlay split assigns the two halves to two different owners on purpose: the base config
(`EdgionPlugins` and friends) is git-owned and carries the *logic*; the EdgionConfigData overlay
carries only *data* and exists so EdgionCenter can push and hot-swap it. EdgionConfigData bodies
are not kept in the ops repository. A GitOps reconciler and the Center dashboard therefore do not
hold competing intents over the same object in the intended deployment model, so an arbitration
rule has nothing to arbitrate. This is why the original finding's Option A (implement ownership)
and Option B (declare last-writer-wins as the contract) were both the wrong frame — the answer is
that the kind has one writer.

**2. The label was never a mechanism, so "only two of four values are implemented" understated it — zero are.**

`rg 'managed_by::|labels::managed_by'` returns no hits anywhere in the workspace. The
EdgionConfigData handler (`resource_processor/handlers/edgion_config_data.rs`) has no label logic;
its reconcile path stores the spec, requeues dependents, and writes status, and never mutates a
spec. The `operator` value's documented meaning — "controller never mutates spec" — was therefore
a description of ambient default behavior, not a rule anything enforced, which is why it looked
"active". Note the scoping: the *reconcile* path never mutates a spec, while the Controller's own
Admin API failover endpoint (`edgion-controller/src/api/region_route_handlers.rs`) does — so the
old `operator` row was not merely unenforced, it was wrong. The label's only live writers put it
on `Secret` and the ACME-generated `EdgionTls`, via their own local string literals rather than
the constants: `acme` (`edgion-controller/src/services/acme/service.rs`,
`edgion-controller/src/services/acme/secret_io.rs`) and `conf-sync-ca`
(`edgion-controller/src/services/conf_sync_ca/secret_io.rs`) — the latter a fifth value the
documented four-value vocabulary omitted entirely.

**3. The competing design was already rejected upstream; write safety is a CAS property, not an ownership epoch.**

The label-based ownership design that would have consumed `center` — Center stamping
`managed-by: center`, refusing to take over an unowned object, and surfacing an
`unowned-conflict` drift state — lived in EdgionCenter's
`tasks/pending/global-resource-management/` package. That package was **superseded and rejected on
2026-07-26** in favour of `tasks/pending/center-controller-interaction-convergence/`, and its own
header says "do not execute its task ledger". The current program authority,
`EdgionCenter/docs/superpowers/specs/2026-07-26-center-controller-write-model-convergence-design.md`,
declares no `managed-by` mechanism at all. Center's cross-cluster `GlobalResources` view is
read-only; Center's two *dedicated* write paths are RegionRouteOverride failover and row-level
copy — the only production callers of `config_data_ops::write_config_data`, whose shared core is
what distinguishes them (the dashboard's generic CRUD sends `If-Match` too, so CAS is not the
difference) — while general EdgionConfigData create / update / delete stays available through the
generic Controller Admin proxy, which is exactly what the default federation policy grants. Correctness throughout
rests on `If-Match` against `metadata.resourceVersion` with explicit `converged` / `superseded` /
`conflict` outcomes. Reintroducing a label-based owner would add a second, coarser concurrency
model beside a CAS model that already works.

**4. There is no cheap enforcement point even if one were wanted.**

The controller's reconcile path observes an object only *after* it is persisted, so it cannot
reject a non-owning writer — it could only write the object back, which turns a lost update into
an unbounded fight between two writers. The only place that can actually refuse a write before it
lands is an admission webhook, and this repository's ingest is deliberately lenient (ingest
reports on a status condition; real enforcement happens later, at Gateway compile time — see
[`../ctx-set/claims-access-log-filter.md`](../ctx-set/claims-access-log-filter.md) for the
established two-part pattern). Option A was therefore a much larger change than its "the
vocabulary already exists" framing suggested.

## Fix Suggestions Not Accepted

- **Honor the label at ingest / implement `center` and `controller`** — no competing writer exists
  by design, and the enforcement point would require a strict admission path that contradicts this
  repository's lenient-ingest convention.
- **Delete the `managed_by` constants outright** — they are unreferenced, but the final shape of
  any provenance keys belongs to the EdgionCenter write-model program, not to a narrow review fix.
  The doc comments now state plainly that they have no readers and no writers.
- **Record "last-writer-wins is the contract" for this kind** — accurate for the Controller Admin
  API in general (that is finding 16's subject, about preconditions being opt-in for *all*
  callers), but the wrong thing to say here: it implies a contended object where the design has a
  single writer.
- **Add an ownership epoch / generation-based arbitration field** — a second concurrency model
  beside the working CAS model, with nothing to protect.

## Re-evaluation Triggers

Re-open this decision only if:

- EdgionConfigData bodies start being stored in the ops repository, or any second writer of the
  overlay is introduced (a GitOps reconciler, a second Center, or an operator-facing authoring
  flow that is expected to survive a Center write).
- The EdgionCenter write-model program reintroduces durable Center-owned desired state for
  EdgionConfigData with plan/apply semantics — the rejected `global-resource-management` shape.
- A reader for `edgion.io/managed-by` is added anywhere. The label is currently provenance only;
  the first reader turns it into a contract and this entry must be rewritten.

## Reference Cases

- `EdgionCenter/tasks/new_work/00-index.md` §15 (closed 2026-07-28; the finding asked for a
  decision between implementing ownership and declaring concurrent authority — both declined in
  favour of "one intended writer"). The per-issue file `15-edgion-config-data-ownership.md` was
  deleted on close, per that index's "a task file that no longer exists in this directory is done"
  convention; the index section is the surviving record.
- Sibling finding 16 (Controller CAS precondition is opt-in for non-Center callers) is a *different*
  problem with a different answer; it concerns the Admin API's promise to every caller, not
  ownership of this kind.
- [`../../01-architecture/05-resources/15-edgion-config-data.md`](../../01-architecture/05-resources/15-edgion-config-data.md) — Ownership section.
- [`../../02-features/05-annotations/00-annotations-overview.md`](../../02-features/05-annotations/00-annotations-overview.md) — the label's actual vocabulary (ACME / conf-sync-CA `Secret`s plus the ACME-generated `EdgionTls`).
- `edgion-resources/src/constants/labels.rs` — `managed_by` module (declarations, no callers).
