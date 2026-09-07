---
name: btp-section-key-requeue-gap
description: Use when reviewing findings that route-side BackendTLSPolicy requeue helpers look up BTPs by the bare "ns/svc" key and miss sectionName-scoped "ns/svc::section" target registrations; covers the base-key reduction fix.
status: fixed
finding_id: btp-section-key-requeue-gap
closed: 2026-07-18
knowledge_type: regression-guards
---

# Route→BTP Requeue Helpers Miss sectionName-Keyed Target Registrations — FIXED

## Conclusion

The two route-side requeue helpers in
`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/mod.rs`
(`requeue_backend_tls_policies_for_route` and its companion
`requeue_backend_tls_policies_for_services`) previously looked up BackendTLSPolicies
with an **exact** `target_mgr().get_refs("ns/svc")` call. BTP target registrations can
carry a section suffix — `make_target_conflict_key(ns, name, Some(section))` produces
`"ns/svc::section"` (`backend_tls_policy_ref.rs`) — so an exact lookup on the bare
service key never matched a section-scoped registration. A route change (or a coalesced
delete→recreate) that dropped a backend Service therefore did **not** requeue a BTP that
targets that Service with a `sectionName`, leaving that BTP's ancestor resolution stale
until an unrelated trigger touched it.

Fixed by reducing every registered target key to its base form before matching, via a
shared `btp_refs_for_service` helper (the same `split("::")` base-key reduction the EBTP
requeue block in `backend_tls_policy.rs` already uses). Re-reports of the "bare-key lookup
misses section registrations" form are **not accepted**.

## Core Rationale

**1. The target manager key space is base-or-section, but the reverse chain feeds only base keys.**

BTP conflict/target keys are `"ns/svc"` OR `"ns/svc::sectionName"`. The route→service reverse
chain (`ServiceRefManager`) only knows the bare `"ns/svc"` service key, so a bare `get_refs`
is structurally incapable of reaching a section-scoped registration. The fix enumerates the
forward index (`all_source_keys()`), reduces each key to `k.split("::").next()`, and matches
on base equality — covering both bare and section registrations in one pass.

**2. This mirrors the already-shipped EBTP requeue reduction.**

The sibling block at `backend_tls_policy.rs` (`base_key = |k| k.split("::").next().unwrap_or(k)`)
established the base-key reduction pattern for the EBTP conflict index. Applying the identical
reduction on the route side keeps the two directions consistent; the shared helper documents the
reason so a future reader does not "simplify" it back to an exact lookup.

**3. Over-match is impossible; extra requeues are harmless.**

Base equality is exact after the suffix strip, and Kubernetes namespace/name tokens cannot contain
`"::"`, so `"ns/svc"` never matches `"ns/svc-other"` or `"ns/svc-other::s"`. Any BTP with several
targetRefs for the same Service across sections yields duplicate refs, but requeue is idempotent
(the workqueue dedups by key), so duplicates cost nothing.

## Fix Suggestions Not Accepted

- "Add a second bare-key registration alongside every section key so `get_refs` works" — duplicates
  the target index and corrupts conflict-precedence counting (`is_winner` would over-count siblings).
- "Strip the section on the write side so only bare keys are ever stored" — sectionName is a real
  Gateway API conflict-scoping dimension (`test_different_section_names_no_conflict`); collapsing it
  would make two different-section BTPs falsely conflict.

## Re-evaluation Triggers

Re-open only if:

- The BTP target key format changes (a different separator than `::`, or an additional key dimension),
  in which case `btp_refs_for_service` and the EBTP `base_key` closure must be updated together.
- The route→service reverse chain starts tracking section scope, at which point an exact section-aware
  lookup could replace the base-key scan.

## Reference Cases

- `tasks/todo/btp-section-key-requeue-gap.md` (closed 2026-07-18) — filed from the on_delete-workqueue
  whole-branch review as the residual section-keyed instance of the coalescing audit's C2 class.
- Fix: `handlers/mod.rs` `btp_refs_for_service` + its two callers; unit tests
  `btp_refs_for_service_matches_bare_and_section_keys` / `btp_refs_for_service_no_match_is_empty`.
- Sibling reduction: `handlers/backend_tls_policy.rs` EBTP requeue block (`split("::")` base key).
- Key format: `backend_tls_policy_ref.rs::make_target_conflict_key`.

## Resolution references

- `btp-section-key-requeue-gap` — 2026-07-18.
