# System-Managed And Test-Only Keys

Use this file when a key is reserved, injected by the system, or only intended for tooling/tests.

## Reserved System-Managed Annotations

| Key | Writer | Meaning | Guidance |
|-----|--------|---------|----------|
| `edgion.io/hostname-resolution` | controller route handlers | Diagnostic summary of effective hostname resolution for `HTTPRoute` / `GRPCRoute` | Do not hand-author. Treat as system-reserved. |
| `edgion.io/sync-version` | Gateway sync/meta layer | Correlates data-plane state with control-plane sync version | Do not hand-author in manifests. |

## Test And Tooling Keys

| Key | Writer / Consumer | Where It Appears | Guidance |
|-----|-------------------|------------------|----------|
| `edgion.io/metrics-test-key` | Gateway metrics test helpers | `Gateway.metadata.annotations` | Integration-testing only. |
| `edgion.io/metrics-test-type` | Gateway metrics test helpers | `Gateway.metadata.annotations` | Integration-testing only. |
| `edgion.io/skip-load-validation` | `examples/code/validator/config_load_validator.rs` | example YAML metadata annotations | Lets config-load validation skip a resource. Do not use as normal product config. |
| `edgion.io/force-sync` | `examples/k8stest/scripts/run_k8s_integration.sh` | temporary annotation on Secrets | Test harness workaround to trigger UPDATE events and dependent requeue. |
| `edgion.io/trigger` | ACME user/admin operations | `EdgionAcme.metadata.annotations` | Manual ACME re-trigger knob. Operational use only. |

## ACME Internal Labels

These are labels, but they often show up during troubleshooting and are easy to mistake for user-facing config:

- `edgion.io/managed-by=acme`
- `edgion.io/acme-resource=<name>`

They are created by the ACME service when persisting account/cert related Secrets.

## Review Guidance

- If a manifest change proposes one of these keys in day-to-day feature config, question it.
- If the task is about observability or debugging, it is often correct to read these keys but not to author them.
- When documenting them, clearly label them as reserved, internal, or test-only.
