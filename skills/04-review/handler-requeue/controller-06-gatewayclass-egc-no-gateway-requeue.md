---
name: controller-06-gatewayclass-egc-no-gateway-requeue
description: Use when reviewing findings that EdgionGatewayConfig / GatewayClass handlers lack on_change/on_delete and "do not requeue dependent Gateways"; covers why this is intentional and why a requeue alone is a no-op.
---

# GatewayClass / EdgionGatewayConfig Do Not Requeue Dependent Gateways — DESIGN-TRADEOFF

## Conclusion

`EdgionGatewayConfigHandler` and `GatewayClassHandler`
(`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/{edgion_gateway_config.rs, gateway_class.rs}`)
intentionally have no `on_change` / `on_delete`, so a change to either resource does **not** requeue
the Gateways that reference them. This is correct as-is. The finding's factual premise (no requeue)
is true, but its causal claims (stale Gateway status / stale cached view / stale federation count)
and its suggested fix (add `on_change`/`on_delete` that requeue Gateways) are wrong: a requeue here
would be a **no-op**, because the controller-side Gateway handler derives no state from either
resource.

Fix suggestions of the form "add `on_change`/`on_delete` to `EdgionGatewayConfigHandler` /
`GatewayClassHandler` that `requeue_all_keys()` the Gateways" or "introduce a `GatewayConfigRefManager`
analogous to `SecretRefManager`" are **not accepted**.

## Core Rationale

**1. The data plane already consumes both resources directly via gRPC sync — functional behavior is correct.**

`EdgionGatewayConfig` and `GatewayClass` are each independently synced resource kinds with their own
gateway-side stores (`EDGION_GATEWAY_CONFIG_STORE` + `ConfHandler` in
`src/core/gateway/config/edgion_gateway/conf_handler_impl.rs`; the GatewayClass store likewise). The
data plane reads them at listener-build time via `config_client.get_gateway_class()` /
`get_edgion_gateway_config()` (`src/core/gateway/runtime/server/base.rs:110-129`). When either resource
changes, the gateway receives the gRPC event and rebuilds its listener config (timeout / retry /
connection settings) from the new values. No controller-side Gateway requeue is needed for the config
to take effect.

**2. The controller-side Gateway handler derives no status from these resources — a requeue changes nothing.**

`GatewayHandler::{parse, validate, update_status}` (`handlers/gateway.rs`) never reads
`gateway_class.spec.parameters_ref` or any `EdgionGatewayConfig`. Gateway status conditions are
computed only from listener validation, TLS Secret refs, port conflicts, and the stream-plugin
annotation ref. Re-running `parse`/`update_status` for a Gateway (which is all a requeue does) cannot
produce `ResolvedRefs=False` / `Accepted=False` for a broken or deleted `parametersRef` chain, because
no code path inspects that chain on the Gateway. The cached processed Gateway likewise contains no
EdgionGatewayConfig-derived state, so it is never stale with respect to these resources; federation
counts Gateway resources, not config-derived state, so they are unaffected as well.

**3. parametersRef errors are already visible — on the GatewayClass, which is where Gateway API puts them.**

`GatewayClassHandler::validate` rejects a `parametersRef` whose group/kind is not
`edgion.io/EdgionGatewayConfig` and stamps `Accepted=False` on the **GatewayClass's own** status. Per
the Gateway API model, `parametersRef` validity is reflected on the GatewayClass status, not propagated
to the Gateway; there is no conformance requirement for a Gateway-level condition mirroring GatewayClass
acceptance. Operators see the broken ref via `kubectl get gatewayclass -o yaml`.

## Fix Suggestions Not Accepted

- "Add `on_change`/`on_delete` that `requeue_all_keys()` the Gateways" — no-op for the claimed benefits
  (status / cache / federation), because the Gateway handler reads neither resource; adds churn and a
  new cross-resource requeue path (GatewayClass→Gateway, EdgionGatewayConfig→Gateway) with no payoff.
- "Introduce a `GatewayConfigRefManager` analogous to `SecretRefManager`" — same no-op problem, plus the
  cost of a new bidirectional ref-tracking subsystem for a dependency that the data plane already
  resolves directly.
- "Surface `ResolvedRefs=False` on the Gateway when the class/config is broken" — this is a new
  status-propagation feature (resolve parametersRef → EdgionGatewayConfig → derive a new Gateway
  condition), beyond Gateway API conformance; not a bug fix. If genuinely wanted, it belongs in
  `tasks/todo/` via the brainstorming workflow, not in a review-issue requeue patch.

## Re-evaluation Triggers

Re-open this decision only if:

- The Gateway handler gains real status-derivation logic that reads `parametersRef` / EdgionGatewayConfig
  (at that point a requeue would no longer be a no-op and the registration rule would apply).
- A concrete operator requirement or conformance test mandates a Gateway-level condition mirroring
  GatewayClass/EdgionGatewayConfig validity — handle as a new feature task, not a requeue patch.

## Reference Cases

- `tasks/review-envoy/controller-06-egc-gc-no-downstream-requeue.md` (closed 2026-05-29, ⚠️ accepted tradeoff).
- Requeue inventory: `skills/01-architecture/01-controller/06-requeue-mechanism.md` (GatewayClass /
  EdgionGatewayConfig rows: "referenced by Gateway; no requeue").
- Data-plane consumption: `src/core/gateway/runtime/server/base.rs:110-129`.
- Contrast with the genuine cascading-requeue case: `handler-requeue-rule.md` (handlers that call
  `lookup_gateway()` / `get_secret()` in `parse()` MUST register and requeue — these two do not).
