---
id: ARCH-008
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-242
independent_verification: confirmed
---

# Borrow precompiled AI backend lookup keys during request planning

## Current ownership and concrete cost

`ai/http/proxy.rs::ai_candidate_spec` formats a new namespace/name String when
projecting a permitted compiled AI backend into a store lookup. The cold route
generation already owns its typed identity in
`CompiledBackendTarget::EdgionBackend(PolicyTargetKey)` in `backends/runtime.rs`.
Unlike `CompiledServiceTarget`, that variant has no precompiled lookup key.

A normal positive-weight permitted backend on the successful planning path
causes three key-formatting constructions across distinct passes: captured
backend contract validation, availability/invalid-weight admission, and candidate
construction. The first two keys are temporary inspection strings; the third
becomes the retained AiPlanCandidate backend key. Candidate preparation also
unconditionally clones that key into temporary passthrough inputs, even when
OnUnknownModel is not Passthrough. Composite validation and model listing have
further projections. `AiBackendSnapshot::get_by_key` accepts a borrowed str.

Adding another consistency check or planner pass recreates this derived identity
on the request path. The lookup representation remains owned by orchestration
despite canonical configuration/generation ownership belonging to the compiled
target. Allocation and formatting are observed source facts; throughput impact
is unmeasured.

## Bounded improvement and legitimate complexity

Derive the AI lookup key once in the canonical compiled backend target, analogous
to the Service target, and expose it by reference. Let ai_candidate_spec borrow
the key for inspections and store lookups. Keep typed PolicyTargetKey
authoritative and derive the cached representation from it. Preserve owned keys
where a ready attempt plan needs their lifetime.

Separate validation, admission and candidate passes have different contracts.
Combining them could alter invalid-weight shares, captured provider/TPM checks,
model-route promotion or continuation fencing. No pass removal is proposed.
A small compiled target wrapper would require updating match_unit.rs,
selected.rs and pg_upstream_peer.rs consumers. Adding another backend registry
would create unnecessary generation ownership. Keeping current formatting is
simpler but retains the repeat work and split representation ownership.

## Invariants, risks and future validation

Preserve permitted-AI checks and dispatch authorization rechecks, raw declared
slots and invalid-weight shares, denied/missing/zero-weight entries, namespace
defaulting, slot occurrence identity, model-route promotion and captured backend
generations. Preserve raw single-backend/single-credential continuation fences,
lookup order and missing-entry outcomes. Leave request BodyStorage, converter
budgets, retries, credentials, terminal accounting and plugin order with their
existing owners.

Future validation should assert derived-key equality and borrowed reuse and
retain denial, missing-runtime share, captured-skew, model-list and continuation
regressions. Use existing integration topology for runtime failover checks.
No extra network fixture or owned request copy is justified for mocks. An actual
implementation must prove borrowed lifetimes and update canonical target
consumers together; no implementation or throughput validation is claimed here.

## Original context and review status

The analyst read original AI routing/quota indexes and native Responses,
Messages, failover, requested-model quota, throttle, backend ReferenceGrant and
LB wrapper-name records. Actual compiled target/store/planning consumers and
existing source tests were inspected. No original decision establishes repeated
key formatting as required.

This root is separate from ARCH-001 feature capability growth, ARCH-005 transport
metadata and ARCH-006 HMAC size parsing. Exact primary/supporting source bindings
are recorded in STATE.json and partition reports. Independent verification is
pending; no production files or tests were modified or executed.
