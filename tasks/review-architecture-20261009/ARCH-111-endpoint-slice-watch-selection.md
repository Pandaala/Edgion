---
id: ARCH-111
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-400
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Apply existing watch selection to EndpointSlice creation

## Current ownership and concrete witness

KubernetesConfig::no_watch_kinds documents suppression of probing and watcher creation and exclusion from processor readiness. The current feature and Kubernetes lifecycle originals require the same. version_detection::other_known_kind_names explicitly recognizes EndpointSlice. KubernetesCenter passes the configured set to KubernetesController; both serving modes compute expected processor inventories through all_processor_kinds. controller.rs:752–769 filters disabled provider kinds, including EndpointSlice, and the existing should_watch helper guards surrounding Secret, ConfigMap and Service creation.

run_controllers at controller.rs:903–914 nevertheless logs and spawns EndpointSlice unconditionally. The generic spawn helper at88–139 synchronously constructs/registers its processor before returning the tracked ResourceController future. This future uses the ordinary namespaced source, raw store and serial worker.

With no_watch_kinds:[EndpointSlice], readiness and expected inventory exclude it while actual processor/watcher creation includes it. Two startup consumers apply the same existing selection differently. There is no original documented mandatory-EndpointSlice exception. This is a source-level configuration witness, not an API incident, startup timeout or universal routing outage; startup may continue despite the unwanted watcher.

## Bounded improvement and legitimate alternatives

Guard the existing registration log and spawn with self.should_watch("EndpointSlice"), matching adjacent providers. Preserve default enabled creation, handler, ResourceController, namespace mode and tracked child ownership. No new spawn registry, runtime dispatch, queue or public abstraction is warranted. An intended mandatory kind would instead require explicit configuration rejection and original contract changes; current schema, recognized inventory, readiness and operator documents support the local correction.

Preserve one workqueue/one serial worker per enabled kind, key-only runtime events, one-time committed-union bootstrap, source-scoped relists, metadata filtering, selected source versions, descendant cancellation/drain and inline status IO. Namespace remains a separate required first-snapshot gate even with all processor kinds disabled; do not weaken its phase gate.

## Evidence, verification and limits

Future coverage must drive the actual startup branch using the existing in-process kube Client transport style: disable provider kinds including EndpointSlice, poll to the first phase wait and establish the runtime registry has no EndpointSlice processor. Also cover enabled creation and inventory agreement; helper-only should_watch testing misses the bypass. Cancel and drain spawned descendants with no network/API server.

Actual full source-lifecycle tests cover GatewayClass labels, Namespace publication/refresh, disabled Gateway, malformed Namespace identity and required-child failure. Namespace tests cover sorted/deduplicated fixed scopes; caches-ready tests protect the independent Namespace/phase gate. None drives disabled EndpointSlice creation. Original source-scoped snapshots and single-worker initialization constrain retained behavior, not permission to spawn a disabled kind.

ARCH-081 Etcd diagnostics and ARCH-088/094 gauges are distinct. No overlapping EndpointSlice no-watch finding was found. No leadership/metrics/TLS/secret/schema/Gateway body or observability change is proposed. Static source/test-body analysis only; no tests, builds, network or application execution occurred. Analyst/root cause: /root/verify_ip_union, independent decision pending.
