# System-Managed And Test-Only Keys

Use this file when a key is reserved, injected by the system, or only intended for tooling/tests.

## Reserved System-Managed Annotations

| Key | Writer | Meaning | Guidance |
|-----|--------|---------|----------|
| `edgion.io/hostname-resolution` | controller route handlers | Diagnostic summary of effective hostname resolution for `HTTPRoute` / `GRPCRoute` | Do not hand-author. Treat as system-reserved. |
| `edgion.io/sync-version` | Gateway sync/meta layer | Correlates data-plane state with control-plane sync version | Do not hand-author in manifests. |
| `edgion.io/dns-resolved-at` | Controller DNS resolver | RFC3339 freshness stamp on a synthetic ExternalName EndpointSlice | Do not hand-author. |
| `edgion.io/controller-ca-source` | Controller CA material wrapper | Exact `ConfigMap` source kind on synthetic CA Secret material inside resolved transport | Internal only; never author in manifests. Ordinary fetched Secrets have the key removed regardless of value. |

The CA source marker preserves ConfigMap-versus-Secret identity in the existing
resolved TLS transport. Its authenticity relies on the trusted Controller
clear-before-resolve boundary; the annotation itself is not an authorization
credential. Genuine ConfigMap wrappers preserve it during global inheritance.

## Test And Tooling Keys

| Key | Writer / Consumer | Where It Appears | Guidance |
|-----|-------------------|------------------|----------|
| `edgion.io/metrics-test-key` | Gateway metrics test helpers | `Gateway.metadata.annotations` | Integration-testing only. |
| `edgion.io/metrics-test-type` | Gateway metrics test helpers | `Gateway.metadata.annotations` | Integration-testing only. |
| `edgion.io/skip-load-validation` | `edgion-tests/src/validator/config_load_validator.rs` | example YAML metadata annotations | Lets config-load validation skip a resource. Do not use as normal product config. |
| `edgion.io/force-sync` | `edgion-tests/k8s/scripts/run_k8s_integration.sh` | temporary annotation on Secrets | Test harness workaround to trigger UPDATE events and dependent requeue. |

There is no parsed `edgion.io/trigger` annotation. Trigger ACME explicitly with
`POST /api/v1/services/acme/{namespace}/{name}/trigger`; do not document an arbitrary
metadata edit as a product annotation contract.

## ACME Internal Labels

These are labels, but they often show up during troubleshooting and are easy to mistake for user-facing config:

- `edgion.io/managed-by=acme`
- `edgion.io/acme-resource=<name>`

They are created by the ACME service when persisting account/cert related Secrets.

## Review Guidance

- If a manifest change proposes one of these keys in day-to-day feature config, question it.
- If the task is about observability or debugging, it is often correct to read these keys but not to author them.
- When documenting them, clearly label them as reserved, internal, or test-only.
