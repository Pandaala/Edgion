---
name: gatewayclass-reporting-and-publication
description: Review GatewayClass reporting coverage, deployment identities, and status-only publication boundaries.
knowledge_type: regression-guards
---

# GatewayClass reporting is independent of publication

## Conclusion and scope

GatewayClass reporting and publication cover every class matching the deployment's
resolved controllerName. Each matching class receives ordinary parameter validation
and is published to ServerCache; each Gateway resolves its own class's parametersRef.
A foreign controllerName is ignored. The obsolete Kubernetes startup key
`gateway_class` is rejected explicitly, including null; there is no name selector.

Independent deployments must use distinct controllerName values. Replicas sharing
one identity must share their configuration view and election
domain. The user explicitly chose this boundary for
`gatewayclass-status-reporting-ownership`, instead of introducing an assignment
registry and coordinated cross-deployment reporter.

Kubernetes can derive an omitted identity from POD_NAMESPACE and the explicitly
injected stable EDGION_CONTROLLER_DEPLOYMENT_NAME. Pod names are never identity
inputs. FileSystem/etcd require explicit controller_name; all modes reject blank
or invalid values at startup. FS/etcd retain controller-name-only class selection.

## Rationale and evidence

The original handler filter required both controllerName and selected class name,
so the processor returned before computing status for an unassigned matching class.
The currentStatus projection did not resolve this earlier filter.

Canonical authority: [GatewayClass architecture](../../../01-architecture/05-resources/02-gateway-class.md),
[status management](../../../01-architecture/01-controller/10-status-management.md),
and the vendored Gateway API v1.5 GatewayClass status/Accepted contract. The pinned
contract requires reporting on all classes specifying the implementation's name
and True/False according to provisioning support; permanent Pending is insufficient.

`GatewayClassHandler::filter` and `should_publish` use the same controllerName
ownership predicate. ResourceProcessor still separates the persistence payload
from cache publication and withdraws objects that lose eligibility through normal
deletion hooks. Class publication/unpublication requeues dependent raw Gateway keys,
including Gateways never admitted to the processed cache.
Gateway serving still requires the existing raw/published class UID proof.
There is no second queue, status worker, API retry, or request-path mechanism.

Kubernetes `cluster_watcher_config` exempts GatewayClass from the global label
selector. `requeue_all_keys` includes raw class keys on leader acquisition.
The canonical conditions-and-fields comparator includes supportedFeatures;
explicit empty-list serialization actually clears stale features in Merge Patch.

The September 2026 Gateway API version work changed `SupportedVersion` from
unconditional `True` to `Unknown/CompatibilityUnknown`: the Controller checks
served GVK availability but does not discover and verify the installed CRD
bundle/channel. Upstream's implementer guide defines `True` for a supported
installed CRD version and `False` for an unsupported one; current evidence
establishes neither conclusion for a live installation. The handler replaces
a stale `True` condition, updates observedGeneration, and preserves foreign
conditions. The offline release fixture matrix is bounded OpenAPI/model
evidence and does not make `SupportedVersion=True` truthful for a cluster.

## Alternatives and limits

Sharing one controllerName across independent deployments would allow opposing fresh writers. CAS prevents
stale snapshots, not repeated disagreement on current resourceVersions. Distinct
identities avoid that overlap under the supported deployment contract; explicit
identity collisions are not detected or prevented at runtime. Generated identities
also assume the injected namespace/Deployment pair is accurate and stable.
Identity generation does not change election Lease names/namespaces; independent
Kubernetes deployments must configure distinct election domains as well.

GatewayClass.spec.controllerName must exactly match the resolved value and is
immutable in Kubernetes. Explicit no_watch_kinds still disables class reporting.
Non-Kubernetes identity validation does not make FileSystem safe for multiple
writers or create distributed coordination for independent etcd election domains.

The former single-class deployment design was permitted by Gateway API; the
`kubernetes-gateway-class-allow-list` change is an authorized feature expansion,
not a correction of a specification violation. Creating another matching class
makes it eligible without restarting the Controller. RBAC must govern class
creation and controllerName assignment. Multiple classes do not authorize sharing
one physical listener port across classes; existing conflict handling remains.

## Verification and re-evaluation

Regression coverage: `handlers/tests/gateway_ownership_test.rs` covers two matching
classes with distinct parametersRef values, foreign non-publication, conditions,
features-only drift, stale-cache withdrawal, recreation, raw-key leader recovery,
independent identities, and multiple Gateways per class.
`kubernetes/tests/status_persistence_test.rs` drives the real status
patch helper through an in-memory Merge Patch transport and checks WATCH convergence.
Watcher selector tests and startup/identity tests cover configuration boundaries.

Current implementation checks for `kubernetes-gateway-class-allow-list`:
Controller resource_processor 832 passed, configuration 42 passed, Kubernetes
configuration center 74 passed; three sequential Controller all-target compile
checks, formatting and diff whitespace checks passed. These are in-process
checks, not live Kubernetes or conformance evidence.

The historical 2026-09-21 broader Controller library run found five failures (1541 passed,
5 failed, 6 ignored). All five were independently reproduced on unchanged HEAD
`8d3d681875afb72af437340e556cf738e848cc38` in a temporary worktree. They remain
outside this correction as the `linksys-timeout-advisory-test-fixtures` and
`filesystem-repaired-resource-error-status` follow-ups.
Neither is treated as corrected or accepted by this fix.

Historical verification for the reporting correction on 2026-09-21:

- Controller resource_processor suite: 825 passed.
- Controller Kubernetes configuration-center suite: 73 passed.
- Controller config::conf_center suite: 42 passed (overlaps the broader run).
- `cargo fmt --all` and final formatting check: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed; only the existing Gateway
  Kafka err/expect and DSL gRPC redundant-async-block test warnings remain.
- Agent-doc validation, SSA force, metrics inventory, Pingora source policy and
  Gateway API transform contract guards: passed.
- Changed shell/Python/config syntax checks, rendered isolated-proxy identity
  consistency and `run_k8s_proxy_protocol_jobs_test.sh`: passed.
- Independent read-only code/knowledge review and final fixture review: LGTM.

The original live reporting script tested rejection and non-publication of a
second matching class. Those historical expectations are superseded by the
controllerName-only publication contract; they are not current verification.

Re-evaluate on controller identity/default changes, new publication hooks,
watch-selector or leader-recovery changes, or any proposal to support independent
deployments sharing controllerName. Such support needs authoritative assignment
and reporter handoff semantics, not a weaker ownership predicate.

## Resolution references

- `gatewayclass-status-reporting-ownership`: separate class status coverage from
  publication under the distinct-deployment identity contract; the single-class
  publication restriction from that correction is superseded.
- `kubernetes-gateway-class-allow-list`: publish all matching classes, preserve
  incarnation proof, and reject the retired startup selector.
