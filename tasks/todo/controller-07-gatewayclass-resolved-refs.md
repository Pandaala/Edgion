---
id: controller-07
title: GatewayClass handler — set ResolvedRefs condition and validate parametersRef target existence
severity: high
origin: tasks/review-envoy/controller-07-gatewayclass-no-resolved-refs.md (promoted to task 2026-05-29)
related: controller-06 (EdgionGatewayConfig/GatewayClass → dependent requeue) — hard prerequisite, see below
---

# GatewayClass: ResolvedRefs condition + parametersRef existence check

Promoted from the `tasks/review-envoy/` finding via the fix-review-issue scope gate
(wide-impact / cross-resource coupling). The original review file has been deleted;
this task carries the full context.

## The gap

`GatewayClassHandler`
(`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs`):

1. `validate()` only checks `parametersRef.group == "edgion.io"` and
   `parametersRef.kind == "EdgionGatewayConfig"`. It never looks up the named
   `EdgionGatewayConfig` to confirm it exists.
2. `update_status()` writes only `Accepted` and `SupportedVersion` conditions. It
   never writes a `ResolvedRefs` condition.

Consequence: a `GatewayClass` whose `parametersRef.name` points at a non-existent
`EdgionGatewayConfig` shows `Accepted=True` (group/kind are valid), appearing healthy.
Meanwhile the data plane skips building the Gateway entirely
(`src/core/gateway/runtime/server/base.rs:110-131`: `None => { warn!; continue }` — it
does NOT fall back to defaults; defaults are only used when `parametersRef` is **absent**).
The Gateway is silently non-functional with no status signal.

Gateway API spec (GatewayClass status, v1): if `parametersRef` is set and the target
does not exist, the controller MUST set `Accepted=False` (reason `InvalidParameters`)
and SHOULD set `ResolvedRefs=False` (reason `InvalidParameters`). Envoy Gateway and
Istio both surface `ResolvedRefs` on GatewayClass.

## Desired behavior

In `update_status()`:
- `parametersRef` absent → omit `ResolvedRefs` (nothing to resolve).
- `parametersRef` present + group/kind valid + target exists → `ResolvedRefs=True`.
- `parametersRef` present + target missing → `ResolvedRefs=False` (reason `InvalidParameters`)
  and, per spec MUST, `Accepted=False` (reason `InvalidParameters`).
- Consider moving the group/kind check out of `validate()` into `update_status()` so all
  reference-resolution logic lives in one place (avoid a double-check).

## Why this was promoted, not fixed inline (the controller-06 coupling)

The clean fix has a hard prerequisite that puts it beyond the narrow fix-review-issue
workflow:

- There is **no `EdgionGatewayConfig → GatewayClass` runtime requeue** today. That is
  exactly what the still-open `controller-06` tracks.
- If `ResolvedRefs=False` / `Accepted=False` is written when the EGC is missing, but no
  requeue exists, then once the EGC is later created the GatewayClass is **never
  re-processed** and the False condition goes permanently stale — more misleading than
  having no condition at all. Init-ordering (GatewayClass processed before EGC) hits the
  same trap.

→ controller-07 should be implemented **together with controller-06** (or controller-06
first), so the new conditions are kept fresh by a dependency requeue.

## Implementation notes (already verified in code)

- `update_status()` and `validate()` are **synchronous** trait methods — no async needed
  for the lookup.
- `EdgionGatewayConfig` is **cluster-scoped** (`#[kube(namespaced = false)]` in
  `src/types/resources/edgion_gateway_config.rs`), so its cache key is just the name =
  `params_ref.name` (key format: `"namespace/name"` for namespaced, `"name"` for
  cluster-scoped).
- Cross-kind existence lookup path (synchronous):
  `ctx.runtime().processor_registry.get("EdgionGatewayConfig")` → `Arc<dyn ProcessorObj>`
  → `.get_raw_any(&params_ref.name)` (returns `Option<Arc<dyn Any + Send + Sync>>`;
  `.is_some()` is enough for an existence check, or `.downcast::<EdgionGatewayConfig>()`
  for the value). NOTE: the original finding suggested a `ServerCache<EdgionGatewayConfig>`
  on `ControllerRuntime` — that does **not** exist; the processor registry is the access
  path.
- Condition helpers exist in `status_utils.rs`: `condition_types::RESOLVED_REFS`,
  `condition_true` / `condition_false`, `update_condition`. There is currently **no**
  `InvalidParameters` reason constant in `condition_reasons` — add one
  (`pub const INVALID_PARAMETERS: &str = "InvalidParameters";`).
- For the requeue side (controller-06 territory): either iterate all Gateways via
  `processor.requeue_all_keys()` on EGC/GatewayClass change, or introduce a
  `GatewayConfigRefManager` analogous to `SecretRefManager`. Also register the
  GatewayClass→EGC dependency so a late/updated EGC requeues the GatewayClass to flip
  `ResolvedRefs` False→True.

## Testing

- Behavioral change (status conditions) → add an integration scenario per
  `skills/05-testing/01-integration-testing.md`: GatewayClass with `parametersRef.name`
  pointing at a missing EGC → expect `ResolvedRefs=False` / `Accepted=False`
  (reason `InvalidParameters`); then create the EGC → expect transition to True
  (this part validates the controller-06 requeue).
- Update the requeue inventory `skills/01-architecture/01-controller/06-requeue-mechanism.md`
  (currently lists GatewayClass / EdgionGatewayConfig as "no requeue").
