---
name: status-list-merge-patch-tombstone
description: Use when reviewing controller status-persistence findings; covers which status list fields must always-serialize so a JSON Merge Patch can clear them, and which skip_serializing_if omissions are safe.
status: fixed
finding_id: merge-patch-cannot-clear-status-lists
closed: 2026-07-20
knowledge_type: regression-guards
---

# Merge-patch cannot clear skip-serialized status lists — FIXED (routes) / AUDITED (rest)

## Conclusion

K8s-mode status persistence uses `Patch::Merge` with serialized `status` and a raw-input
`metadata.resourceVersion` precondition
(`persist_k8s_status`, `edgion-controller/.../kubernetes/resource_controller.rs`).
Under RFC 7386 a `{"status": {}}` body is a no-op and a key is deleted only by an
explicit `null`; arrays are replaced wholesale. So a **bare `Vec` status list
field** annotated `#[serde(skip_serializing_if = "Vec::is_empty")]` can never
transition non-empty → empty on a live object — once empty it is omitted, the
patch degenerates to `{"status": {}}`, and the stale list survives forever (the
status-diff gate re-detects a diff and re-issues the same no-op patch every
reconcile).

The six route-family `parents` fields are **fixed** (always-serialize). Every
other status list on this path was audited and is safe for a concrete reason
below. Re-reports of the form "field X is also skip-serialized so it can't be
cleared" are **not accepted** unless they show a *reachable* non-empty → empty
transition for a field not in the audit table.

## Core Rationale

**1. Fixed: route-family `parents` (reachable → empty)**

`HTTPRouteStatus` / `GRPCRouteStatus` / `TCPRouteStatus` / `TLSRouteStatus` /
`UDPRouteStatus` / `EdgionTlsStatus` each hold only `parents: Vec<RouteParentStatus>`.
The handlers reach empty via `else { status.parents.clear() }` (all parentRefs
removed) and `retain_current_parent_statuses` — the route object survives, so the
empty transition is real. Fixed by dropping `skip_serializing_if` and keeping
`#[serde(default)]` + a do-not-reintroduce doc comment, mirroring the EBTP/BackendTLSPolicy
`ancestors` fix (2026-07-19). Pinned by `route_status_empty_parents_serialize_explicit_array`
in `http_route.rs` (covers all six via `{"parents": []}`).

**2. Safe — `Option<Vec>` set to `Some(empty)`, never `None`**

`GatewayStatus.listeners` / `.addresses` / `.conditions` and
`GatewayClassStatus.supported_features` use `skip_serializing_if = "Option::is_none"`.
The Gateway handler writes `status.listeners = Some(Vec::new())` on "all listeners
removed" (`handlers/gateway.rs`), and addresses is always `Some(non_empty)`. A
`Some(empty)` serializes as an explicit `[]`, which merge-patch replaces correctly.
`supported_features` is a static non-empty list, always `Some`.

**3. Safe — `conditions` never empties**

`GatewayClass` / `EdgionGatewayConfig` / `LinkSys` / `EdgionAcme` `conditions:
Vec<Condition>` are `skip_serializing_if = "Vec::is_empty"`, but the controller
always sets ≥1 condition (`Accepted`) and never clears them, so the empty
transition is unreachable.

**4. ACME clears challenges through its dedicated writer**

`active_challenges: Vec` is skip-serialized *intentionally*: it is written by a
dedicated field manager (`edgion-acme-challenges`) via
`patch_active_challenges` (`services/acme/service.rs`), which builds raw JSON with
an **explicit** `"activeChallenges": active` array (empty `[]` clears it) using
`Patch::Apply`. The skip keeps `update_acme_status`'s freshly constructed empty
challenge list out of its SSA payload (field manager `edgion-acme-controller`).
It does not omit a nonempty list observed by the generic worker. The earlier claim
that skip-empty alone prevents generic-worker clobbering was incorrect: concurrency
safety comes from the shared raw-input resourceVersion precondition, described in the
[snapshot concurrency guard](../../status/regression-guards/kubernetes-status-snapshot-concurrency.md).
Removing the skip would make the generic worker explicitly clear observed empty lists
and make the lifecycle SSA writer submit a field owned by the challenge writer.

## Fix Suggestions Not Accepted

- "Also always-serialize every `conditions` list" — unreachable empty transition; adds churn for no benefit.
- "Remove `skip_serializing_if` from `EdgionAcme.active_challenges`" — makes the lifecycle SSA payload submit the challenge writer's field; explicit clearing already belongs to `patch_active_challenges`.
- "Emit an explicit `null` tombstone in the persist path instead of always-serializing" — a per-field `[]` at the struct is simpler, local, and testable; a generic null-injector in `persist_k8s_status` cannot know which keys are lists.

## Re-evaluation Triggers

Re-open only if:

- A new status struct adds a bare `Vec` list field with `skip_serializing_if = "Vec::is_empty"` that can reach empty while the object survives (add it to the fix, mirror the pattern).
- The Gateway handler is changed to set `status.listeners`/`addresses` to `None` (rather than `Some(empty)`) after having been populated — that would make the `Option::is_none` skip reachable.
- The ACME challenge writer stops sending an explicit array, or the field managers are unified.

## Reference Cases

- `tasks/todo/merge-patch-cannot-clear-status-lists.md` (2026-07-20 audit + fix; task file closed on completion)
- Precedent: EBTP/BackendTLSPolicy `ancestors` always-serialize fix (2026-07-19), `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`
- Persist path: `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs` (`persist_k8s_status`), `extract_status_value` (`resource_processor/processor.rs`)

## Resolution references

- `merge-patch-cannot-clear-status-lists` — 2026-07-20.
