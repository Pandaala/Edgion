# B0218 — Gateway Runtime Handler

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/handler.rs`.
- Baseline: `aa178a25`.
- Analyst: `b0211_final_review`, acting only as the B0218 analyst.
- Investigate full/delta Gateway snapshot publication, derived listener state and
  all route-family consumers without duplicating queued B0217 or known issues.
- No implementation authorized; existing integration and user-owned paths remain protected.

## Analysis Outcome

- Decision: deferred. Target blob `b58bad4f465909d4a9f7a1cd6a7d4ec2584c6a2f` at `aa178a25`; analyst freshly passed PROXY policy tests 12/12, which do not cover the new handler failure.
- D0123: same-key compressed delete/add produces add-only final data; prospective policy sees replacement B, identity store receives B, but add_gateway rejects existing A. Rebuilt PGIS/TLS matcher retain A until another event repairs it. Source-proven medium persistent convergence defect, not accepted transient publication or a demonstrated exploit.
- Reproduce with unchanged socket semantics rather than assuming fatal policy bypass. Plan-first correction must preserve compression, align accepted prospective/final generation, test local delta paths and retain unchanged-listener rebuild avoidance. Do not redesign all stores or broaden B0217.
- D0003 full-set omitted identity removal remains separate; its broad partial-event claim was narrowed with D0123 evidence. No low-value logging/collection cleanup used to obscure the publication issue.
- Analyst confirms unrelated semantics-preserving optimization can safely continue. Exact later mutation/test support paths require plan approval; new wire coverage remains deferred.
