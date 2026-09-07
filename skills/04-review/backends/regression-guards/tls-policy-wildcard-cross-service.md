---
name: tls-policy-wildcard-cross-service
description: BackendTLSPolicy sectionName matching now filters targetRefs by tr.name == queried service before testing sectionName, so a multi-Service policy can no longer leak another Service's wildcard/scoped ref. Reject re-reports claiming the wildcard or exact match bleeds across services.
knowledge_type: regression-guards
---

# backends-F006 — BackendTLSPolicy sectionName match isolated per targetRef Service

**Status:** ✅ fixed · 2026-05-24
**Severity:** Low (BadLogic) — wrong TLS scope applied across ports of a Service; requires an exotic multi-Service policy; no crash / data corruption

## Conclusion

`query_backend_tls_policy_for_service` (`edgion-gateway/src/backends/policy/mod.rs`) now selects
through `select_policy_for_service`, whose exact-match and wildcard (no-`sectionName`) closures
both require `tr.name == name` **before** testing `section_name`. A policy that references several
Services with mixed `sectionName` values can therefore no longer have a ref aimed at a *different*
Service satisfy the match for the queried Service.

Re-reports of the form "wildcard (no `section_name`) match `.any()` ranges over all `target_refs`
and bleeds across services", "a `svc-b/None` ref makes the policy the all-ports fallback for
`svc-a`", or "exact `sectionName` match fires on another Service's ref" are **not accepted** — none
are true anymore. Verify the current closures carry the `tr.name == name` guard before re-reporting.

## What was wrong (before the fix)

The reverse index (`build_reverse_index`, `backend_tls_policy_store.rs`) stores a policy in **every**
referenced Service's bucket (one entry per `targetRef`). The query then ran
`pol.spec.target_refs.iter().any(|tr| tr.section_name.is_none())` (wildcard) and
`...any(|tr| tr.section_name.as_deref() == Some(pn))` (exact) over **all** of a policy's refs. So a
policy with `targetRef[svc-a] = Some("https")` + `targetRef[svc-b] = None`, when queried for `svc-a`,
would let the `svc-b` (no-`sectionName`) ref win the whole-Service slot for `svc-a` — even though its
only `svc-a` connection is the `"https"`-scoped ref. The exact branch had the same defect symmetrically
(a `svc-b/grpc` ref matching a `svc-a` port-`grpc` query).

The misselection only changes the result when a competing **correct** candidate exists for the queried
Service (the final `candidates.first()` fallback masks the single-candidate case), which is why the
regression tests use a two-candidate list.

## Why the fix is correct

A policy lands in the `query_ns/name` reverse-index bucket only when one of its refs satisfies
`policy_ns/ref.name == query_ns/name`, i.e. `policy_ns == query_ns && ref.name == name`. So every
candidate already shares the queried namespace, and isolating the refs that actually point at the
queried Service needs **only** a `tr.name == name` check — no `format!`-rebuilt `ns/name` key (which the
finding's suggested fix proposed). This matters because both callers (`pg_upstream_peer.rs`,
grpc `integration.rs`) run this per-request on the data-plane hot path; the name-only filter is
allocation-free.

## Fix Suggestions Not Accepted

- "Rebuild the full `ns/name` key with `format!` inside the closures and compare" — correct but adds a
  per-request, per-targetRef heap allocation on the hot path; the `tr.name == name` filter is equivalent
  (namespace already guaranteed equal by the bucketing) and allocation-free.
- "Only fix the wildcard branch; the exact branch is just for consistency" — the exact branch is a real
  bug of the same class (another Service's scoped ref matching this Service's port query), so both are
  fixed.
- Broadening the fix to the final `candidates.first()` fallback (a `sectionName`-scoped-only policy
  returned for a non-matching port) is **out of scope** for this finding — it is a separate matter from
  the cross-Service `.any()` leak F006 reports.

## Re-evaluation Triggers

Re-open only if:

- The reverse-index keying stops guaranteeing `policy_ns == query_ns` for returned candidates (e.g.
  cross-namespace targetRefs become allowed), which would invalidate the name-only filter.

## Reference Cases

- backends-F006 (review finding, fixed 2026-05-24)
- `select_policy_for_service` unit tests in `edgion-gateway/src/backends/policy/mod.rs`
  (`wildcard_ignores_other_service_ref`, `exact_match_ignores_other_service_ref`, plus two
  no-regression guards).
