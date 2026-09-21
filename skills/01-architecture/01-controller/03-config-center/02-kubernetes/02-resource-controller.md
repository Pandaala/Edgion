---
name: kubernetes-resource-controller
description: Kubernetes ResourceController lifecycle — source snapshots, union bootstrap, one runtime worker, and conditional status writeback.
---

# ResourceController Lifecycle

Each kind has one `ResourceController<K>`, one `ResourceProcessor<K>` and one workqueue
worker. Different kinds run independently. Multiple namespace watchers feed the same
kind's controller; they do not create additional processors or workers.

## Sources and bootstrap

`run_namespaced()` builds one source for all namespaces, one selected namespace, or
each distinct namespace in a configured list. `run_cluster_scoped()` builds one cluster
source. Both enter `run_sources(inputs)`; the former `run_with_api` path is gone.

Each source owns a native reflector store through `SourceState`. `KubeRawStore` exposes
one store or the nonoverlapping namespace-store union and is injected before processing.
It represents raw API objects, including those later rejected by business filters.

1. Sources collect `Init` / `InitApply` and commit complete snapshots at `InitDone`.
2. Wait until every source has committed once; snapshot the complete raw union.
3. Call `processor.on_init()`, then `run_init_apply_once` for every snapshot object.
   Sources are not polled during replay, so lookups see the complete union.
4. Each replay captures the raw resourceVersion, calls `on_init_apply`, then awaits
   conditional status persistence if changed and leadership permits.
5. Call `on_init_done()` and spawn exactly one worker.

Initial watch changes contribute to the committed snapshot; there is no separate pre-init
`Apply` persistence entrypoint. Duplicate completion does not create a second worker.

## Runtime events and source relists

After bootstrap, `SourceState::apply` updates its source and returns affected keys.
`run_sources` sends every returned key through `processor.on_event_key` for the existing
worker, including deletes. The worker fetches the current raw object at dequeue time,
captures its resourceVersion, and calls `process_work_item`. Coalesced events therefore
reconcile current state rather than replaying obsolete object bodies.

A successful source relist replaces that source's complete snapshot and enqueues current
keys (including unchanged resourceVersions) plus removed keys. Other sources retain their
snapshots; ServerCache and the worker are not reset. A source error retains its last
committed snapshot and uses source-local backoff. A 410 Gone followed by LIST uses this
relist flow, not a global Relink rebuild. Resuming WATCH without LIST does not force resync.

Required source EOF or worker termination fails the controller epoch; it is not silently
treated as a healthy smaller source set. Cancellation/shutdown stops the worker and drops
the sources. See [lifecycle](00-lifecycle.md) for the surrounding epoch contract.

## ResourceProcessor pipeline

`ResourceProcessor::process_resource` applies namespace/business filters, cleans metadata,
validates/preparses/parses, computes status, finalizes transport, semantically compares
status, notifies dependents through `on_change`, and publishes to ServerCache. Status persistence
is a subsequent Kubernetes-controller responsibility, not an API call inside a handler.
The raw input version is captured before this processing and must not be derived from
processed metadata or sync-version annotations.

Cross-resource requeues go through the processor registry. `on_change` precedes
`save_to_cache`; a handler's self-lookup uses RawStore, not its prior ServerCache entry.
See [status management](../../10-status-management.md) for derivation and semantic diff
and [workqueue](../../04-workqueue.md) for deduplication and trigger chains.

## Conditional status writeback

```rust
async fn persist_k8s_status<K>(
    client: &Client,
    api_scope: &ApiScope,
    namespace: Option<&str>,
    name: &str,
    resource_version: Option<&str>,
    status_value: &serde_json::Value,
) -> Result<(), StatusWriteError>
```

The generic function uses `DynamicObject` with K's canonical GVK and plural, selecting
the namespaced or cluster-scoped `/status` endpoint from `ApiScope`. GatewayClass and
EdgionGatewayConfig both use the cluster-scoped path. All status-computing kinds and future
`ResourceController` consumers inherit the same behavior without an opt-in list.

There are exactly two persistence entrypoints:

| Entrypoint | Input version | Write gate |
|---|---|---|
| `run_init_apply_once` | Initial complete raw union object, before processing | `status_changed` and leadership |
| `spawn_worker` | Raw object fetched for the dequeued key, before processing | `status_changed` and leadership |

`leader_handle.as_ref().is_none_or(|h| h.is_leader())` supplies the leadership gate.
Kubernetes lifecycle wiring supplies the handle. The write uses one JSON Merge Patch:

```json
{"metadata":{"resourceVersion":"raw-input-version"},"status":{"parents":[]}}
```

The version is opaque and must be nonempty. Missing/empty versions produce a local
`StatusWriteError::MissingResourceVersion` before any request. Kubernetes rejects stale
versions atomically: an intervening successful status, spec, or metadata update prevents
an old snapshot from replacing newer fields/arrays. Never GET a fresh version and attach
it to stale computed status, or fall back to an unconditional write.

Arrays still replace wholesale: handlers preserve foreign entries in the input snapshot
and explicitly serialize empty arrays when clearing owned lists. The version precondition
does not replace ownership rules, establish field-manager ownership, or fence leadership.
ACME's field-scoped SSA service writers remain separate.

Existing success/error metrics cover this path. Callers treat 404 as already deleted;
other failures, including 409 and missing versions, are warned without immediate retry or
failure-triggered requeue. Status IO remains inline in the serial worker. Later events,
successful source LISTs, dependency requeues or leadership-acquisition requeues provide
another opportunity, with no bounded recovery guarantee. See the
[no-retry decision](../../../../04-review/handler-requeue/intentional-design/status-write-no-retry-accepted.md)
and [snapshot concurrency guard](../../../../04-review/status/regression-guards/kubernetes-status-snapshot-concurrency.md).

## Key files

- `edgion-controller/src/conf_mgr/conf_center/kubernetes/resource_controller.rs` — lifecycle, worker, status persistence
- `edgion-controller/src/conf_mgr/conf_center/kubernetes/watch_source.rs` — source snapshots
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs` — processing and publication
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handler.rs` — handler contract
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_diff.rs` — semantic comparison
- `edgion-controller/src/conf_mgr/sync_runtime/workqueue.rs` — serial workqueue
- `edgion-controller/src/conf_mgr/processor_registry.rs` — cross-resource dispatch
