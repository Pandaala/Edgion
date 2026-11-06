# res-03: EdgionGatewayConfig mixes per-GatewayClass fields with process-global fields aggregated over every EGC in the cluster

- **Category**: design-error
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `edgion-resources/src/resources/edgion_gateway_config.rs:93` (`link_sys`), `:103` (`load_balancing`), `:107` (`outbound_tls`), `:114` (`dns_resolver`)
  - `edgion-resources/src/resources/gateway_class.rs:36` (`parameters_ref` binds one EGC per class)
  - `edgion-gateway/src/config/edgion_gateway/conf_handler_impl.rs:450-506` (`recompute_global_config` over the whole store), `:395-403` (`get_egc_dns_resolver_default`: lowest key wins)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs:99-170` (every EGC is processed and published; no binding filter)

## Summary

`EdgionGatewayConfig` is a cluster-scoped object that a `GatewayClass` selects through `parametersRef`, so an
operator reads it as "the config of this class". Most of its fields behave that way (bound through
`bind_gateway_config(name)`), but five of them are applied as process-global values computed across *all*
EGC objects synced to the gateway — including EGCs that no GatewayClass references and EGCs of other classes:
`linkSys.webhookMaxResponseBytes` (min), `linkSys.maxInstancesPerKind` (min),
`loadBalancing.degradeThreshold` (max), `outboundTls` (safest-wins / conflict fallbacks), and the
`dnsResolver` default (lowest object name wins). The resource schema gives no indication of this split.

## Current behavior (evidence)

- The Controller EGC handler validates and publishes every EGC; it never checks whether a served
  GatewayClass references it. conf_sync broadcasts all of them.
- `recompute_global_config(store)` folds `store.values()`:
  - webhook cap and LinkSys instance cap → `min()`; LB degrade threshold → `max()` (`:448-477`);
  - `GlobalOutboundTlsSnapshot::from_configs(store.values())` — when two EGCs declare CA defaults the CA falls
    back to System; when two declare client identities "inherited outbound TLS is disabled" (`:479-500`);
  - DNS resolver default chosen by lowest EGC key (`:394-398`).
- These values feed process-wide runtime singletons (`configure_webhook_global_max`,
  `configure_link_sys_max_instances_per_kind`, `configure_lb_degrade_default`, `set_outbound_global_default`).
- Operator docs describe the fields per EGC (`skills/02-features/02-config/02-edgion-gateway-config.md:246,
  318-319`) but do not say they are folded across unrelated EGCs; the fixed regression guard
  `edgion-gateway-config-global-aggregation-deterministic` only made the fold order-independent.

## Industry comparison

- Envoy Gateway: `EnvoyProxy` (parametersRef of a GatewayClass or Gateway) is strictly scoped to the proxies it
  parameterizes; there is no cross-object min/max. Truly process-wide knobs live in the
  `EnvoyGateway` startup config, not in a namespaced/cluster CR.
- Istio: mesh-wide defaults are in one `MeshConfig`; per-workload overrides are in `ProxyConfig`/annotations
  with explicit precedence, never an implicit fold over all objects.

## Impact

- Cross-class interference: a staging EGC (or an orphan left behind) with `degradeThreshold: 100` turns on
  degrade mode for every Service of every class; a smaller `webhookMaxResponseBytes` silently truncates/rejects
  other classes' webhook calls.
- A second EGC that declares an outbound client certificate silently disables inherited outbound mTLS for all
  LinkSys/plugin clients in the process (fail-closed outage), visible only as a WARN/metric.
- The DNS default switches when someone creates an EGC whose name sorts lower.
- Nothing on any EGC or GatewayClass status explains why the effective value differs from the bound EGC.

## Suggested direction

Option A (recommended): move the process-global knobs out of the CRD into the gateway process config
(`edgion-gateway` YAML, already used for other process-global plugin limits — see
`skills/02-features/02-config/01-gateway-config.md` "metrics and process-global plugin limits"). The EGC keeps
only fields that can genuinely vary per GatewayClass.

Option B: keep them in EGC but aggregate only EGCs bound to a GatewayClass this controller serves, and report
the effective value and any conflict as a condition (`EffectiveGlobalOverridden`/`Conflicted`) on every
contributing EGC.

Option C: make them truly per-class (per-class LinkSys stores / LB defaults), which is larger work and probably
not justified.

## Notes

- Reviewed `skills/04-review/resources/regression-guards/edgion-gateway-config-global-aggregation-deterministic.md`
  (determinism and delete reset, not scope semantics) and `skills/01-architecture/05-resources/03-edgion-gateway-config.md`.
  No intentional-design page states that unbound EGCs are supposed to influence process globals.
- Gateway-side code is cited for evidence; the defect is in the resource contract (where these fields live).

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `conf_handler_impl.rs:445-500` `recompute_global_config` folds `store.values()` (every synced EGC, bound or not): webhook cap and instance cap use `min`, degrade threshold uses `max`, and `GlobalOutboundTlsSnapshot::from_configs` disables inherited client TLS on a conflict. The DNS default is chosen by lowest key (`:390-398`).
- `skills/01-architecture/05-resources/03-edgion-gateway-config.md:109-115` states that LinkSys, LB, outbound TLS and DNS "keep their existing global dynamic consumers" while plugin policy is GatewayClass-scoped (unreferenced EGCs do not contribute). This confirms the split without justifying folding unbound EGCs.
- `resources/regression-guards/edgion-gateway-config-global-aggregation-deterministic.md` is about order-independence and delete reset only.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `recompute_global_config` (`conf_handler_impl.rs:450-506`) still folds `store.values()` with `min`/`max` and `GlobalOutboundTlsSnapshot::from_configs`; DNS default still lowest key (`:395-403`). Line refs updated.
- Branch adds `spec.downstreamTls` to EGC, resolved per `ResolvedGatewayConfig`; the architecture doc notes its ECDH curves are loaded through a startup-bound handle with "GatewayClass retargeting limitations unchanged", adding another field whose scope is not per-class, but nothing narrows the existing global fold.
