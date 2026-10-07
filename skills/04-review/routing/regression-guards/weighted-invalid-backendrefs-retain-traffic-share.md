---
knowledge_type: regression-guards
---

# Weighted Invalid BackendRefs Retain Their Traffic Share

## Required failure share

Invalid backendRef slots retain their authored order and weight. Silently removing
invalid slots or redistributing their share to healthy siblings defeats the
weighted-invalid-reference contract. Target validation and Controller dependency
registration must not compact the backend vectors. Invalid references produce no
new target dependency or authorization/store read; that does not remove their
failure slots from the effective rule.

## Current classification and dispatch evidence

The canonical `refs::targets::is_standard_service` accepts only omitted/empty group
and omitted/exact `Service` kind. Literal `core`, ServiceImport, ServiceExternalName,
and other pseudo kinds classify as Invalid, retaining their weight. The presence
of an older `EdgionService::from_kind` arm does not establish that a native reference
can reach it. A standard Service object's actual type is a separate peer-resolution
concern; this page makes no reachability claim for those older pseudo-kind arms.

At the R1 source inspection, `CompiledBackend::is_generic_selectable` in
`routes/http/match_unit.rs` includes compiled Service, EdgionBackend, and Invalid
variants, and `build_backend_selector` retains their original indices. Thus a valid
AI target is not excluded from that selector. The ordinary peer path in
`pg_upstream_peer.rs` rejects a selected EdgionBackend with
`UpstreamNotBackendRefs` and 503; AI dispatch has its separate plan and requires
permitted backend references. These are observed call-chain facts, not a new
authorization guarantee inferred from selector test counts.

Controller AI facts preserve one entry per authored/effective slot. Invalid or
Grant-denied targets become `Missing` without reading Service/EdgionBackend state;
valid denied references still retain recovery dependencies and RefNotPermitted
status. Target-only fact loans never grant permission themselves.

## Historical evidence and re-evaluation

The `gw-12` resolution (2026-08-09) preserved weighted-invalid failure shares. Its
older description of core ServiceImport as a valid Service target, and valid AI
being excluded from the ordinary selector, is superseded by the current source
classification and selection evidence above. Historical WRR counts are not fresh
verification and do not override the reference contract.

Re-evaluate when changing canonical target classification, selector membership,
invalid-slot weighting, AI dispatch, or ordinary peer rejection. Trace current
source and tests before applying this page to a new finding.
