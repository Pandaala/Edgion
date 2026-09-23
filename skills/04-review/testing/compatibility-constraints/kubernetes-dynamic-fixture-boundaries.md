---
name: kubernetes-dynamic-fixture-boundaries
description: Use when reviewing Kubernetes dynamic scenario scope exceptions and their fixture boundaries.
knowledge_type: compatibility-constraints
---

# Kubernetes dynamic fixture boundaries

## Current constraints

`AcmeGatewayRefOwnership` seeds `status.activeChallenges` in YAML and changes those values across phases. The Kubernetes EdgionAcme CRD enables a `/status` subresource. Ordinary resource apply and the Controller Admin create/update path do not write that status, while the live ACME controller may update it independently. The current Kubernetes runner has no isolated status seed mechanism. The local scenario remains active; the Kubernetes scope exception does not assert equivalent coverage.

`EncryptedActiveHealthChecks` still needs its local certificate generations and
listener topology in Kubernetes. Its scope exception does not claim equivalent
coverage.

## Mitigation and removal conditions

Each exception is confined to its literal dynamic scenario ID in
`k8s_suite_scope_exceptions.tsv`. The parity validator checks source evidence
before accepting it. Remove the ACME exception after the harness can seed and
preserve challenge status through an authorized `/status` path without racing
the live controller. Remove the active health exception when the same
certificate generations and reachable listeners are supplied. Then execute
the original phase assertions.

## Evidence and limits

- `config/crd/edgion-crd/edgion_acme_crd.yaml` — `activeChallenges` is a status field and the CRD enables the status subresource.
- `edgion-tests/src/client/dynamic/scenarios/acme_gatewayref_ownership.rs` — phase updates use ordinary fixture apply.
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/storage.rs` — Admin create and update use ordinary resource operations.
- `edgion-tests/src/client/dynamic/scenarios/encrypted_active_health_checks.rs` — certificate and listener requirements.

These are current test-harness limitations, not product-runtime acceptance decisions. Re-evaluate if the fixture, CRD, Admin API, or Kubernetes runner gains the required capabilities.

The former ExtProc and private GatewayClass boundaries were removed after the
2026-09-23 OrbStack Kubernetes run. The independent Go processor supplied a
Service DNS name, generated CA Secret, TLS listeners, and admin counters;
`ExtProcTlsPolicyRecovery` passed. The Controller accepted both `pp-a` and
`pp-b`, and `PluginPolicyDynamic`, `L4TimeoutHotReload`, and
`PreflightPolicyHotReload` passed their original phase assertions. The
remaining exception rows above retain their narrower fixture constraints.
