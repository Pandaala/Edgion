---
name: selected-waf-session-capability
description: Review the selected-WAF PluginSession integration boundary and transaction transfer semantics.
knowledge_type: intentional-design
---

# Selected-WAF session capability

## Decision and scope

The WAF portion of `ARCH-001` uses one required `PluginSession::waf()` entry
returning `&mut dyn WafSessionCapability`. The feature integration trait lives in
`plugins/runtime/traits/waf_session.rs`; PingoraSessionAdapter implements it
directly and returns itself. Eight feature operations no longer grow the common
session operation list or generated common mock. AI capabilities remain outside
this bounded change; this decision does not close all of ARCH-001.

`WafRequestContext` remains the sole selected-state owner, including the frozen
partial-capture policy from the pinned consumer graph. Reading that policy without
a selected owner returns false without allocating. The policy survives transaction
removal; no-winner selection completion and shared prefix-capture facts remain
independent request state. No view wrapper,
request copy, allocation, lock, registry or generic capability framework is
introduced. Each synchronous capability borrow ends before ordinary session
calls or body-capture awaits. Real native admission remains in the adapter;
phase claims, pinned generation, summaries and accounting retain their owners.

## Contracts and alternatives

Capability methods are required. Production adapters cannot inherit permissive
phase/streaming defaults. Isolated mocks explicitly configure their feature
operations and attach through `expect_waf().return_var(Box<dyn ...>)`; the manual
Redis test adapter panics on unexpected access. No unavailable fallback object
is needed by current consumers.

Transaction installation always consumes the offered transaction. Success
transfers it into the selected context. Rejection destroys only the offer,
retains an existing transaction and returns false; the WAF caller records the
existing TransactionOwnerConflict terminal error and empty-body 500. Returning
a large transaction Result or boxing it adds cost without a current recovery
consumer. Take transfers the matching owner's transaction to the callback;
terminal failure/cancellation cleanup and existing accounting remain unchanged.

A context-only implementation cannot perform native length/stage checks. A view
wrapper adds a carrier without benefit; a PluginSession supertrait makes every
adapter a feature implementation; an extension trait retaining base operations
fails to remove common-list growth. Optional nested borrowed returns conflict
with current mockall support. Keeping the old design was the lower-migration-cost
alternative; this selected design buys a smaller future change surface without
claiming a throughput or memory improvement.

Hard admission and mutation freeze precede native/cached size checks, and the
limit outlives transaction removal and inspection stop. Generic capture APIs,
BodyStorage, independent physical/format/inspection limits, prefix versus
complete-document publication, transport cancellation and access-log ownership
are not redesigned by this boundary.

## Evidence and re-evaluation

Current sources are `runtime/traits/session.rs`, `traits/waf_session.rs`,
`runtime/session_adapter.rs`, `plugins/http/waf/mod.rs` and `ctx.rs`. Focused
coverage includes the feature automock borrow regression, real-adapter election/
stage/freeze and rejected-install resident preservation, existing WAF phase/error
regressions, unchanged ctx summary/admission tests, ExtensionRef log ownership
and plugin-store generation pinning. Fresh execution receipts belong in the
implementation task; historical WAF wire results are not verification of this
refactor. A disposable mockall 0.14/async-trait prototype established object-safe
mutable references, direct borrowed-adapter return and Send async interleaving.

Re-evaluate when native admission ordering, ownership transfer/recovery, adapter
availability, mutable trait-object support, callback cancellation or ctx ownership
changes. Adding an ordinary WAF operation should extend the feature trait,
adapter and WAF consumers/tests without extending PluginSession or its common
mock. A transport-sensitive operation legitimately still needs adapter work.

## Resolution references

- `ARCH-001`: selected-WAF integration boundary only; AI/session debt deferred.
