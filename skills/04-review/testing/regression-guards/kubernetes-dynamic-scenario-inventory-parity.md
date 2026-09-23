---
name: kubernetes-dynamic-scenario-inventory-parity
description: Use when changing local dynamic scenarios or Kubernetes integration execution coverage.
knowledge_type: regression-guards
---

# Kubernetes dynamic scenario inventory parity

## Conclusion and scope

The Kubernetes full integration run executes the IDs in
`k8s_dynamic_scenarios.tsv` through `run_k8s_dynamic_scenarios.sh`, in addition to
the directly invoked AI backend policy scenario. A local dynamic scenario needs
an executable Kubernetes entry or a specific, evidence-backed scope exception in
`k8s_suite_scope_exceptions.tsv`. An entry requires reachable listeners, real
Kubernetes fixtures, and the original scenario assertions; a row alone is not
coverage. The same inventory check also compares static, isolated, and final
gates.

## Rationale and evidence

`test_client.rs::EXPECTED_DYNAMIC_SCENARIO_IDS` is the canonical local ID set.
`validate_suite_parity.sh` compares it with the Kubernetes manifest and direct
calls. The Kubernetes runner invokes the dynamic helper in full-run mode and
fails on an incomplete result protocol. The helper restores each scenario's
declared reset directory, waits for the named primary resource to reach the
Gateway, and invokes `test_client --dynamic-scenario <id>` in the client pod.
The source tasks `k8s-dynamic-scenario-inventory-parity` and
`complete-k8s-dynamic-scenario-parity` recorded missing dynamic IDs. At
implementation time, the live inventory had twelve gaps; a
concurrent HEAD update also introduced `PreflightPolicyHotReload`. The execution
manifest now includes those supported scenarios. The static
`EdgionPlugins/DslTerminate` selection was also added to the Kubernetes suite
array to close the parity gate.

## Boundaries and limits

`AcmeGatewayRefOwnership` remains an explicit exception because its phases
inject challenge ownership and expiry through ACME resource status. Ordinary
Kubernetes apply cannot write the status subresource, and the current helper
has no status patch phase. `ExtProcTlsPolicyRecovery` remains an explicit
exception because it requires an independent Go processor with TLS material
and an admin observation API; the Kubernetes topology only has the built-in
fake processor. These rows do not represent executed Kubernetes assertions.
The pre-existing `EncryptedActiveHealthChecks` exception has a separate local
certificate and listener boundary.

The exact fixture constraints and removal conditions are recorded in
[Kubernetes dynamic fixture boundaries](../compatibility-constraints/kubernetes-dynamic-fixture-boundaries.md).

## Verification and re-evaluation

The initial static inventory and helper checks passed, but they did not execute
the scenarios. A later live `orbstack` run built local arm64 images and passed
`CorsValidation`, `DslTerminateDynamic`, `HttpRouteNamedParameterPrecedence`,
`HttpRoutePathPrefixTrailingSlash`, `LinkSysDisabledTls`,
`LinkSysProviderReplacement`, `RateLimitLocalOverflowDynamic`,
`TlsRouteAttachments`, and the subsequently added `AiGuardOrderingDynamic`.
The static `EdgionPlugins/DslTerminate` suite passed all four assertions after
its Kubernetes HTTP contract fixtures were added. The full prepare could not
finish because the cluster Service IP range filled during bulk fixture apply;
the focused runs used the deployed stack and each scenario's reset fixtures.
The three private GatewayClass scenarios were tried and failed because this
controller deployment selects only `public-gateway`; they are now explicit
scope exceptions rather than executable manifest entries. Re-evaluate each
exception when its stated topology requirement changes. Re-run inventory
parity whenever either runner or manifest changes.

## Resolution references

- `k8s-dynamic-scenario-inventory-parity` — confirmed test coverage gap; supported
  scenarios registered and narrow unsupported boundaries documented.
- `complete-k8s-dynamic-scenario-parity` — the later inventory pass also
  registered supported Kubernetes fixtures and the static DSL terminate suite.
