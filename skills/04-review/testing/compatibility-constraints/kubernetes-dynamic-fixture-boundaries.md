---
name: kubernetes-dynamic-fixture-boundaries
description: Use when reviewing Kubernetes dynamic scenario scope exceptions and their fixture boundaries.
knowledge_type: compatibility-constraints
---

# Kubernetes dynamic fixture boundaries

## Current constraints

`ExtProcTlsPolicyRecovery` requires the independent Go processor, its admin counters, a generated CA Secret, and a Gateway-reachable TLS listener. The processor's `validateListenAddress` currently permits only literal `127.0.0.1` listeners. The Kubernetes harness deploys no instance of that processor, and the Gateway runs in a different Pod. The Rust test server does not provide the independent processor's counter API. The local scenario remains active; the Kubernetes scope exception does not assert equivalent coverage.

`AcmeGatewayRefOwnership` seeds `status.activeChallenges` in YAML and changes those values across phases. The Kubernetes EdgionAcme CRD enables a `/status` subresource. Ordinary resource apply and the Controller Admin create/update path do not write that status, while the live ACME controller may update it independently. The current Kubernetes runner has no isolated status seed mechanism. The local scenario remains active; the Kubernetes scope exception does not assert equivalent coverage.

The Kubernetes Controller deployment selects only the `public-gateway`
GatewayClass. `L4TimeoutHotReload` and `PreflightPolicyHotReload` each require a
private GatewayClass to isolate edits to their EdgionGatewayConfig.
`PluginPolicyDynamic` changes the parameters of a private GatewayClass across
two generations. Live attempts confirmed that their Gateways and listeners
are not published in this topology. Their local assertions remain active; the
Kubernetes exceptions do not claim coverage.

## Mitigation and removal conditions

Each exception is confined to its literal dynamic scenario ID in
`k8s_suite_scope_exceptions.tsv`. The parity validator checks source evidence
before accepting it. Remove the ExtProc exception after a Kubernetes-managed
independent processor supplies a reachable TLS target, generated CA material,
admin endpoint, and equivalent connection/RPC counters. Remove the ACME
exception after the harness can seed and preserve challenge status through an
authorized `/status` path without racing the live controller. Remove the three
GatewayClass exceptions after isolated Controller and Gateway instances can
select their required classes. Then execute the original phase assertions.

## Evidence and limits

- `edgion-tests/integration/fixtures/ext_proc_external/main.go` — `validateListenAddress` enforces loopback listeners.
- `edgion-tests/src/client/dynamic/scenarios/ext_proc_tls_policy_recovery.rs` — fixture identity, CA, target, and counter requirements.
- `config/crd/edgion-crd/edgion_acme_crd.yaml` — `activeChallenges` is a status field and the CRD enables the status subresource.
- `edgion-tests/src/client/dynamic/scenarios/acme_gatewayref_ownership.rs` — phase updates use ordinary fixture apply.
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/storage.rs` — Admin create and update use ordinary resource operations.
- `edgion-tests/k8s/kubernetes/controller/configmap.yaml` — the integration Controller selects `public-gateway`.
- `edgion-tests/k8s/conf/TCPRoute/L4TimeoutHotReload/DynamicTest/initial/02_Gateway_l4-timeout-gw.yaml` and `edgion-tests/k8s/conf/Gateway/PreflightPolicyHotReload/DynamicTest/initial/resources.yaml` — private class references.
- `edgion-tests/src/client/dynamic/scenarios/plugin_policy.rs` — private GatewayClass parameter generations.

These are current test-harness limitations, not product-runtime acceptance decisions. Re-evaluate if the fixture, CRD, Admin API, or Kubernetes runner gains the required capabilities.
