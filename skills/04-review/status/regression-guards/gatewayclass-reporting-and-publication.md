---
name: gatewayclass-reporting-and-publication
description: Review GatewayClass reporting coverage, deployment identities, and status-only publication boundaries.
knowledge_type: regression-guards
---

# GatewayClass reporting is independent of publication

## Conclusion and scope

Kubernetes GatewayClass reporting covers every class matching the deployment's
resolved controllerName. Only the selected class is published to ServerCache;
unselected matching classes receive Accepted=False/Unsupported with an explicit
empty supportedFeatures list. They do not resolve deployment-local parameters.
Old owned ResolvedRefs/SupportedVersion conditions are removed; foreign conditions
and unchanged transition times remain intact. A foreign controllerName is ignored.

Independent deployments must use distinct controllerName values. Replicas sharing
one identity must share their selected class, configuration view and election
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

`GatewayClassHandler::filter` determines reporting eligibility;
`should_publish` retains the selected-class check. ResourceProcessor always returns
the computed persistence payload, but status-only outcomes bypass on_change and
after_publish, withdrawing an old cache object through normal deletion hooks.
Gateway serving still requires the existing raw/published class UID proof.
There is no second queue, status worker, API retry, or request-path mechanism.

Kubernetes `cluster_watcher_config` exempts GatewayClass from the global label
selector. `requeue_all_keys` includes raw class keys on leader acquisition.
The canonical conditions-and-fields comparator includes supportedFeatures;
explicit empty-list serialization actually clears stale features in Merge Patch.

## Alternatives and limits

Simply removing class filtering would allow opposing fresh writers. CAS prevents
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

## Verification and re-evaluation

Regression coverage: `handlers/tests/gateway_ownership_test.rs` covers reporting,
non-publication, foreign conditions, features-only drift, stale-cache withdrawal,
recreation, raw-key leader recovery, independent identities, and multiple Gateways
per class. `kubernetes/tests/status_persistence_test.rs` drives the real status
patch helper through an in-memory Merge Patch transport and checks WATCH convergence.
Watcher selector tests and startup/identity tests cover configuration boundaries.

`edgion-tests/k8s/scripts/test_gatewayclass_reporting.py` is a focused live-cluster
hot-update regression using an existing Controller. It is not a Gateway API
conformance suite. The current local cluster lacks GatewayClass CRDs and Edgion
deployments; no live Kubernetes or conformance pass is claimed.

The broader Controller library run found five existing failures (1541 passed,
5 failed, 6 ignored). All five were independently reproduced on unchanged HEAD
`8d3d681875afb72af437340e556cf738e848cc38` in a temporary worktree. They remain
outside this correction as the `linksys-timeout-advisory-test-fixtures` and
`filesystem-repaired-resource-error-status` follow-ups.
Neither is treated as corrected or accepted by this fix.

Fresh verification for this correction on 2026-09-21:

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

The live script can be run after deploying the tested binary and CRDs, using
`EDGION_ADMIN_TOKEN` and
`python3 edgion-tests/k8s/scripts/test_gatewayclass_reporting.py --gateway-class <selected-class> --controller-url <admin-url>`.
It checks unlabelled class rejection, hot updates, feature clearing, stable echoes,
recreation and absence from publication, without starting a second topology.

Re-evaluate on controller identity/default changes, new publication hooks,
watch-selector or leader-recovery changes, or any proposal to support independent
deployments sharing controllerName. Such support needs authoritative assignment
and reporter handoff semantics, not a weaker ownership predicate.

## Resolution references

- `gatewayclass-status-reporting-ownership`: separate class status coverage from
  selected-class publication under the user-approved distinct-deployment identity contract.
