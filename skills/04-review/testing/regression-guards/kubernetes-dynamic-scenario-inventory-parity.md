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
has no status patch phase. This row does not represent executed Kubernetes assertions.
The pre-existing `EncryptedActiveHealthChecks` exception has a separate local
certificate and listener boundary.

The current implementation adds an independent Go processor Pod with generated
TLS material and an admin observation endpoint, registers
`ExtProcTlsPolicyRecovery`, and registers the three private GatewayClass
scenarios after Controller selection changed to `controller_name`. It also
adds `WasmRuntimeOwnership` fixtures and a Gateway-mounted Wasm test artifact.
On 2026-09-23, current local arm64 Controller, Gateway, and test-client images
passed the six originally missing dynamic scenarios plus `L4TimeoutHotReload`,
`PreflightPolicyHotReload`, and `WasmRuntimeOwnership` in OrbStack Kubernetes.
`PluginPolicyDynamic` exercised distinct `pp-a` and `pp-b` class parameters and
all original phases. The GatewayClass reporting regression passed matching
publication, update, recreation, and foreign-controller isolation. The final
cache-residue gate passed. Controller and Gateway Pod UIDs stayed fixed with
zero restarts across the dynamic phases. The parity inventory passed at 55/55.
The bulk static fixture apply stopped in `Status/` when the local Service IP
range filled; the targeted dynamic runs restored their own initial fixtures
after freeing test-owned, unrelated Services. This live evidence covers the
listed dynamic scenarios, not a full static suite run.
The cache-residue gate deletes configuration objects by design. After it ran,
the test namespaces were removed and the saved four Service specs, two
ConfigMap data maps, two Deployment specs, and four CRD specs were restored
exactly. The original Controller, Gateway, and unrelated Redis Pods became
Ready again. Run cache residue only after other live assertions and restore
from a pre-run snapshot when using an existing developer cluster.

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
The earlier private GatewayClass attempts failed under the former
single-class controller deployment. That result is historical and does not
verify the current multi-class implementation. Re-run inventory parity and
the registered scenarios against current images whenever the runner, manifest,
or Controller class selection changes.

## Resolution references

- `k8s-dynamic-scenario-inventory-parity` — confirmed test coverage gap; supported
  scenarios registered and narrow unsupported boundaries documented.
- `complete-k8s-dynamic-scenario-parity` — the later inventory pass also
  registered supported Kubernetes fixtures and the static DSL terminate suite.
