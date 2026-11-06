# ctrl-center-04: Watch fan-out re-clones and re-serializes every event per gateway per kind across a 4-task pipeline

- **Category**: best-practice
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/conf_sync/cache_server/store.rs:250-275` (`get_events_from_sync_version` clones each `WatcherEvent<T>` per watcher)
  - `edgion-controller/src/conf_sync/cache_server/cache.rs:111-215` (per-watcher task, `mpsc::channel(1)`)
  - `edgion-controller/src/conf_sync/cache_server/cache.rs:295-340` (`watch_json`: second task, `serde_json::to_string(&response.events)` per watcher)
  - `edgion-controller/src/conf_sync/conf_server/config_sync_server.rs:290-345` (`watch`: third task adding `server_id`)
  - `edgion-controller/src/conf_sync/conf_server/grpc_server.rs:640-700` (fourth task: `payload::encode` + 1 s `check_interval`)
  - `edgion-common/src/conf_sync/proto/config_sync.proto` (one `Watch` RPC per kind)

## Summary

List responses are serialized once per version and shared (`list_grpc_cached`), but Watch is fully
per-subscriber: each (gateway × kind) stream owns four spawned tasks chained by three capacity-1
channels plus a 1 s interval timer, and each stream independently deep-clones the same events out of
the ring and runs `serde_json` (and the payload encoder) on them. With G gateways and ~20 synced kinds
the controller runs ≈80·G tasks and 20·G timers, and every event is cloned and serialized G times.

## Current behavior (evidence)

- `EventStore::get_events_from_sync_version` returns `Vec<WatcherEvent<T>>` built with `ev.clone()`
  (full typed resource, e.g. a 1000-endpoint EndpointSlice) for each watcher scan.
- `ServerCache::watch` spawns the watcher task; `watch_json` spawns a converter task that calls
  `serde_json::to_string(&response.events)`; `ConfigSyncServer::watch` spawns an adapter; the gRPC
  `watch` spawns the encoder/reload-poll task. All four hops use `mpsc::channel(1)`.
- The gateway opens one Watch per kind (~20 streams per gateway; the accepted relist-backoff page
  counts 17).
- Identical bytes are produced G times for the same `(from_version, to_version)` window; batches
  differ only in their start version.

## Industry comparison

- Envoy ADS / Delta xDS multiplex all types on one stream per proxy; Istio caches generated resources
  (`XdsCache`) and marshals each resource once per push, reusing the `Any` bytes for every proxy that
  receives it.
- go-control-plane `SnapshotCache` stores pre-marshalled resources; per-stream work is bookkeeping only.

## Impact

Controller CPU/allocation for steady-state churn scales as events × gateways × payload size; a fleet of
hundreds of gateways turns EndpointSlice churn (see ctrl-proc-01/02 and ctrl-center-03) into a dominant controller cost.
Task/timer overhead is paid even when idle. The capacity-1 hop chain also means a slow gateway holds a
cloned event batch per hop.

## Suggested direction

- Short-term, no protocol change: store each event's serialized JSON once (`Bytes`, produced in
  `apply_event` under the write lock or lazily via `OnceCell`) next to the typed value; a watcher batch
  becomes a concatenation of shared `Bytes` slices into a JSON array. Collapse the three adapter tasks
  into the watcher task (they only rename fields and add `server_id`), and fold the 1 s reload poll into
  the same select.
- Medium-term: one multiplexed Watch stream per gateway carrying `(kind, events)` frames
  (ADS-like). This also creates the "single multiplexed stream" precondition the accepted cross-kind
  atomicity pages list as their re-evaluation trigger, without committing to SotW.

## Notes

Checked `conf-sync/intentional-design/ring-buffer-fixed-capacity-toooldversion-relist.md`,
`cross-kind-push-no-snapshot-atomicity-controller-side.md`, `concurrency/intentional-design/reload-detection-polling-vs-event-driven.md`
(accepts the 1 s poll as a backstop; this finding does not contest that, only its per-stream
multiplication) and `regression-guards/bounded-payload-compression.md`. None addresses per-subscriber
re-serialization. No `tasks/` entry.

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P3 (was P2)
- `edgion-controller/src/conf_sync/cache_server/store.rs:262-272`: `events.push(ev.clone())` per watcher scan.
- `edgion-controller/src/conf_sync/cache_server/cache.rs:115,210,302-315`: per-watcher task + `serde_json::to_string(&response.events)` in a second task, `mpsc::channel(1)` hops.
- `config_sync_server.rs:313-315` and `grpc_server.rs:646-658`: third and fourth per-stream tasks.
- Downgraded to P3: real architectural inefficiency, but it scales with G (typically tens) on the control plane only; fix once ctrl-proc-01/02 remove the self-inflicted churn that dominates this cost.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `edgion-controller/src/conf_sync/cache_server/store.rs:268`: `events.push(ev.clone())` per watcher scan; `cache_server/` unchanged since `feature-08-30`.
- `edgion-controller/src/conf_sync/cache_server/cache.rs:315`: per-watcher `serde_json::to_string(&response.events)` still in the converter task.
- `config_sync_server.rs` unchanged; `grpc_server.rs` only changed the `TcpIncoming` constructor, the per-stream encoder task remains.
