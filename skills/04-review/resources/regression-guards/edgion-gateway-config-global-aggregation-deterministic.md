---
name: edgion-gateway-config-global-aggregation-deterministic
description: Use when reviewing EdgionGatewayConfig handler findings that the per-resource apply of a process-global value (webhook cap / LB degrade threshold) is non-deterministic across cluster-scoped instances or is never reset on delete.
status: fixed
finding_id: types-misc-03
closed: 2026-06-14
knowledge_type: regression-guards
---

# EdgionGatewayConfig process-global values aggregated deterministically — FIXED

## Conclusion

`EdgionGatewayConfigHandler` no longer applies a process-global value once per
resource during the store iteration. Both globals are now recomputed from the
**entire** store after every mutation, with an order-independent aggregate and a
reset-to-default when no instance carries the field:

- `linkSys.webhookMaxResponseBytes` → `min` (strictest cap wins), default `16384`.
- `linkSys.maxInstancesPerKind` → `min` (strictest cap wins), default `200`.
- `loadBalancing.degradeThreshold` → `max` (panic engages as early as any asks), default `0`.

Re-reports of the form "per-resource apply is order-dependent" / "delete never
resets the global" / "two cluster-scoped configs flip the value across relist"
are **not accepted** against the current code.

## Core Rationale

**1. Cluster-scoped means multiple instances are valid input.**

`EdgionGatewayConfig` is declared `namespaced = false`
(`edgion-resources/src/resources/edgion_gateway_config.rs`), so the gateway store
(`EDGION_GATEWAY_CONFIG_STORE`) can legitimately hold several instances. The old
code did `for config in store.values() { apply_*(config) }`, writing each config's
value into a single process-global atomic — the surviving value depended on
`HashMap` hash order and could flap across relists. The REMOVE branch only deleted
the store key and never recomputed, so a relaxed cap lingered until process restart.

**2. The fix is a single recompute over the post-mutation store.**

`recompute_global_config(&store)` (in
`edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs`) iterates all
instances, aggregates deterministically, and calls `configure_webhook_global_max`
/ `configure_link_sys_max_instances_per_kind` / `configure_lb_degrade_default`
once. It is invoked at the end of `full_set` and at
the end of `partial_update` (after add/update/remove are all applied), so the
delete path restores the default automatically. The aggregate rule matches each
field's safety direction: caps tighten (`min`), degrade mode is conservative (`max`).

**3. Default constants are pinned to the atomics' initial values.**

`DEFAULT_WEBHOOK_MAX_RESPONSE_BYTES = 16384` mirrors `WEBHOOK_GLOBAL_MAX`'s init
and the schema `default_webhook_max_response_bytes`; `DEFAULT_LINK_SYS_MAX_INSTANCES_PER_KIND
= 200` mirrors `LINK_SYS_MAX_INSTANCES_PER_KIND`'s init and the schema
`LINK_SYS_DEFAULT_MAX_INSTANCES_PER_KIND`; `DEFAULT_LB_DEGRADE_THRESHOLD =
0` mirrors `GLOBAL_DEFAULT` (panic disabled / opt-in). A comment on each constant
records the cross-file invariant.

## Fix Suggestions Not Accepted

- "Make `EdgionGatewayConfig` singleton-only / validate at most one in the controller" — heavier and unnecessary; deterministic aggregation handles multiple instances correctly without an admission constraint.
- "Re-apply per resource but pick the last by name" — still arbitrary operator-intent guessing; `min`/`max` aggregation is the safety-correct, order-independent rule.
- "Keep per-resource apply, just also reset on delete" — leaves the multi-instance non-determinism in `full_set` / batched `partial_update`; only a full-store recompute fixes both halves.

## Re-evaluation Triggers

Re-open only if:

- A new `LinkSysGlobalConfig` / `LoadBalancingConfig` field is added whose correct multi-instance aggregate is neither `min` nor `max` (e.g. a value that should be summed or last-writer) — it must still route through `recompute_global_config`, not a per-resource apply.
- `EdgionGatewayConfig` becomes namespaced or singleton-enforced, making multi-instance input impossible (then the aggregation is harmless but redundant).

## Reference Cases

- `tasks/review-2/types-misc-03-link-sys-global-config-non-deterministic.md` (closed 2026-06-14).
- `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs` — `recompute_global_config`, `full_set`, `partial_update`.
- `docs/en/ops-guide/configuration/edgion-gateway-config.md` — `spec.linkSys` multi-instance aggregation note.
