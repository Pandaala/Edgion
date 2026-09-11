# Bind Outlier State and Gauge Updates to Live Generations

## Meta

- Status: pending
- Severity: medium
- Category: observability-correctness

## Evidence

Request reporting, selector checks, and recovery can retain `Arc<ServiceCbState>` or endpoint handles after the corresponding store entry has been replaced or removed. Configuration replacement, policy removal, Service cleanup, or backend cleanup can drain the currently published generation, after which an in-flight operation on the detached generation can still open or recover an ejection and update the generationless `edgion_circuit_breaker_ejected_endpoints` gauge.

This can increment a gauge for state with no published owner, or race a cleanup drain and decrement the same logical ejection twice. Future lookups use the replacement generation, so the stale metric may have no remaining lifecycle owner.

## Why separate

A correct fix needs one generation-aware ownership and metric protocol across configuration publication, request reporting, lazy recovery, backend cleanup, and Service cleanup. An extra drain or generation check in one branch cannot close all detached-handle races.

## Impact and open questions

- Bind every ejection/recovery metric transition to a live store generation.
- Preserve generation-valid half-open probes and current per-Service percent arbitration.
- Define exactly-once gauge decrement ownership across recovery, replacement, backend removal, and Service removal.
- Keep fixed-cardinality labels and existing selection behavior.
- Coordinate with, but do not merge into, the broader atomic Service-policy snapshot work.

## Candidate next step

Design a live-generation token carried by captured Service/endpoint state. Add deterministic pause/resume tests around failure and recovery publication, then replace/remove the Service or backend and prove detached generations cannot alter current state and every logical gauge increment has exactly one decrement.
