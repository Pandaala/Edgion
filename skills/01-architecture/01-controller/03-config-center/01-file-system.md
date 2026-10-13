---
name: config-center-file-system
description: FileSystemCenter implementation — local YAML directory watching, file naming convention, status persistence, startup scan, and runtime watcher.
---

# FileSystemCenter Implementation

FileSystemCenter watches a recursive local YAML tree and runs changes through the same runtime-owned processing model as Etcd and Kubernetes.

## Architecture

```
FileSystemCenter
├── writer: FileSystemStorage       # CenterApi delegate (CRUD operations)
├── config: FileSystemConfig        # configuration (conf_dir, etc.)
└── base: CenterBase                # ControllerRuntime, ConfigSyncServer slot, reload/lifecycle state

FileSystemController.run()
├── FileSystemWatcher (shared, centralized file watching)
│   ├── [PollWatcher (adaptive/overridden interval)] → [dispatcher] → [kind channels]
│   │                                        ├── HTTPRoute channel
│   │                                        ├── Gateway channel
│   │                                        └── ... other kind channels
│
├── spawn::<HTTPRoute, _>(HttpRouteHandler)
│   ├── ResourceProcessor + register into ControllerRuntime.processor_registry
│   └── FileSystemResourceController (subscribes to its kind channel)
│
├── spawn::<Gateway, _>(GatewayHandler)
│   └── ...
└── registry-defined resource kinds, spawned from the shared non-K8s list
```

## File Naming Convention

The watcher scans recursively. New CRUD writes and supported inputs use the
canonical subdirectories below. Current code can still discover older
locations; that behavior is pre-release removal debt, not a supported layout.
Do not add new fallback locations. `PathRegistry` must choose a deterministic
winner if duplicate files are encountered while that debt remains.

| Resource scope | File-name format | Example |
|----------------|------------------|---------|
| Namespaced | `{root}/{namespace}/{Kind}_{namespace}_{name}.yaml` | `default/HTTPRoute_default_my-route.yaml` |
| Cluster-scoped | `{root}/cluster/{Kind}__{name}.yaml` | `cluster/GatewayClass__edgion.yaml` |

Status files: append a `.status` suffix to the corresponding configuration file name.

| Configuration file | Status file |
|--------------------|-------------|
| `HTTPRoute_default_my-route.yaml` | `HTTPRoute_default_my-route.yaml.status` |
| `GatewayClass__edgion.yaml` | `GatewayClass__edgion.yaml.status` |

Parsing rule: `parse_resource_filename()` splits by the first `_` into at most three segments (`splitn(3, '_')`):
- The first segment is Kind
- If the second segment is empty (double underscore), the resource is cluster-scoped; otherwise it is the namespace
- The third segment is the name

## Status Persistence

FileSystemCenter does not call the K8s API; it writes status to a local `.status` file:

- **Format**: YAML serialization of the native status structure (preserves the full Gateway API status structure)
- **Write timing**: written by ResourceProcessor after processing completes when the status changes
- **Read usage**: status change detection (read and converted to a JSON string for comparison)
- **Error status**: when a resource fails to parse, a simplified `ErrorStatus` is written (`conditions: [{type: Ready, status: False, reason: ParseError}]`)
- **Error-status clearing**: after a repaired resource is applied, a resource that computes no status of its own cannot overwrite that sidecar, so init and the worker delete it explicitly. The `Ready` condition type is the sole sidecar marker, so any other persisted status is left untouched (see [native status sidecar cleanup](../../../04-review/status/regression-guards/native-status-sidecar-cleanup.md))
- **Orphan cleanup**: at startup, `cleanup_orphans()` scans and deletes any `.status` files without a corresponding configuration file. It never removes a sidecar whose configuration file still exists

## Startup Flow

The `start()` method of FileSystemCenter performs the following steps (with reload-loop support):

```
start(shutdown_handle)
└── loop:
    ├── 1. Create iteration_shutdown + reload channel + error channel
    ├── 2. FileSystemController.run()
    │   ├── Clean up orphan .status files
    │   ├── spawn_all_controllers (shared with EtcdController):
    │   │   ├── FileSystemWatcher.prime(): scan dir once → resolve key collisions →
    │   │   │   swap PathRegistry → snapshot files-by-kind (NO events emitted)
    │   │   ├── Phase 1: spawn base resource controllers
    │   │   │   (GatewayClass, Gateway, Secret, ConfigMap, ReferenceGrant, Service, EndpointSlice)
    │   │   ├── dispatch_init(phase1_kinds): emit Init/InitApply/InitDone for Phase-1 kinds from the snapshot
    │   │   ├── wait_kinds_ready(phase1_kinds, 15s)  ← real gate: the init pump has already run,
    │   │   │   so Phase-1 processors reach ready and Phase 2 only starts after (no more dead 15s wait)
    │   │   ├── Phase 2: spawn dependent resource controllers
    │   │   │   (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute, EdgionTls,
    │   │   │    BackendTLSPolicy, EdgionPlugins, EdgionStreamPlugins, EdgionConfigData,
    │   │   │    LinkSys, EdgionAcme, EdgionGatewayConfig, EdgionBackendTrafficPolicy)
    │   │   └── dispatch_init(phase2_kinds): emit init for Phase-2 kinds from the same snapshot
    │   └── FileSystemWatcher.run_runtime()
    │       └── runtime_phase: PollWatcher (adaptive interval) → Apply/Delete dispatched directly
    ├── 3. wait_registry_ready(30s)
    ├── 4. Trigger cross-namespace revalidation
    ├── 5. Create ConfigSyncServer + register all WatchObjs
    ├── 6. set_config_sync_server(Some)  ← gRPC service becomes available
    ├── 7. select! { shutdown / reload / error }
    └── 8. Cleanup: clear ConfigSyncServer, abort controllers, ControllerRuntime::reset()
        ├── Shutdown → return
        ├── Reload → continue loop (new server_id)
        └── Error → wait for shutdown signal then return
```

### Init Phase Details

The init phase is split into a snapshot step (`prime`) and a per-phase dispatch
step (`dispatch_init`), so `spawn_all_controllers` can pump Phase-1 init and gate
on Phase-1 readiness **before** spawning Phase 2 (the K8s backend achieves the
same ordering via its per-kind informers).

`FileSystemWatcher.prime()` performs one synchronous directory scan and stores a
snapshot — it emits **no** events:

1. WalkDir recursively traverses the configuration directory
2. Filter `.yaml` / `.yml` files, exclude `.status` files
3. Read file contents and identify Kind via `ResourceKind::from_content()`
4. Resolve key collisions into a new `PathRegistry` generation and swap it atomically
5. Store the winning `(path, content)` list grouped by Kind for `dispatch_init`

`FileSystemWatcher.dispatch_init(kinds)` replays the snapshot for exactly the
requested kinds:

- For each Kind send `Init` → `InitApply(path, content) * N` → `InitDone` in order
- For a requested Kind with no snapshot entry, still send `Init` → `InitDone` (so all controllers complete initialization and reach ready)

`init_phase()` is retained as a thin `prime()` + `dispatch_init(all subscribed kinds)` wrapper used only by unit tests; production drives the two steps separately from `spawn_all_controllers`.

### Runtime Phase Details

`FileSystemWatcher.runtime_phase()` uses the `PollWatcher` from the `notify` crate:

1. Count YAML files and choose a 1–5 second PollWatcher interval (or use `fs_poll_interval_ms` override), then forward raw events to `mpsc::unbounded_channel`
2. Ignore `EventKind::Access` (read-only access does not trigger processing)
3. The main loop calls `dispatch_file_event` directly for each event, with no application-level debounce —
   PollWatcher already coalesces multiple events on the same path into one within each poll cycle
4. `dispatch_file_event(path, kind)` decides via `infer_dispatch_action`:
   - `Remove(_)` or `Modify(Name(From))` → **Delete**
   - `Create(_)` or `Modify(Name(To))` → **Apply**
   - Other ambiguous events → decide Apply / Recheck based on `path.exists()` (PollWatcher rarely emits ambiguous events; this branch is kept as defense-in-depth)
5. Dispatched per Kind via `broadcast::channel` to the corresponding `FileSystemResourceController`

### Design Decisions and Trade-offs

**Why PollWatcher rather than RecommendedWatcher (FSEvents/inotify)**

The original implementation used `RecommendedWatcher` (FSEvents on macOS, inotify on Linux) and added a 1-second debounce + a `HashMap<PathBuf, EventKind>` collector + `had_genuine_remove` tracking at the application layer to handle event dedup and atomic-replace semantics.

**What we found in practice**: macOS FSEvents **silently drops a large number of events** under high-concurrency batch deletions — deleting 656 files in one batch (completed in 156ms), FSEvents only delivered about 64 Remove events (~90% loss rate), with no `kFSEventStreamEventFlagMustScanSubDirs` rescan flag and no overflow warning from the OS. This caused the `cache_residue` integration test to fail consistently — the runtime store and the gateway client cache could never be cleared.

After switching to `PollWatcher`:
- Never loses events (each poll cycle stat-s the entire directory and diffs against the previous snapshot)
- The application layer no longer needs debounce / pending HashMap / `had_genuine_remove`; about 100 lines of code were removed
- Cross-platform behavior is consistent (PollWatcher is a pure user-space implementation, decoupled from OS push APIs)

**Cost**: polling latency. The default is 1 second through 2,000 files, then
steps by one second per additional 2,000-file bucket and caps at 5 seconds.
`fs_poll_interval_ms` overrides this selection.

**Lower bound on FS-mode latency**

Tests and tools must budget at least the configured/selected poll interval plus
processing time; do not assume a universal 1.5-second bound for large trees.

**Known limitations**

- **Delete-then-immediately-recreate within the same 1s window is coalesced into a single Modify**: PollWatcher only sees snapshot diffs; the intermediate deleted state is invisible. If a ConfHandler relies on diff computation between on_delete / on_change (for example, to clean up reverse references), it must instead compare "old vs new value" inside `parse()` rather than relying on on_delete. See the implementations in `reference_grant.rs` and `backend_tls_policy.rs` for the pattern.
- The runtime PollWatcher establishes its baseline only after `prime()` has scanned, so a file created/changed in the window between the `prime()` scan and the `run_runtime()` baseline is not seen until its next change. This window is now slightly wider — it spans the Phase-1 `dispatch_init` + readiness wait — and remains an **accepted** limitation of the dev/test FS backend. The etcd backend has no equivalent: its watch resumes from the primed revision (`R + 1`) and replays that gap. See `docs/superpowers/specs/2026-07-20-etcd-fs-phase1-ordering-design.md`.

## Phased Initialization

Like KubernetesCenter, FileSystemCenter uses a two-phase initialization:

| Phase | Resources | Description |
|-------|-----------|-------------|
| Phase 1 (base) | GatewayClass, Gateway, Secret, ConfigMap, ReferenceGrant, Service, EndpointSlice | Other resources query these during `parse()` |
| Phase 2 (dependent) | Remaining kinds in `non_kube_common::spawn_all_controllers`, including routes, policies, extensions, ACME, and LinkSys | Started after Phase 1 completes |

Phase 2 waits on `runtime.processor_registry` for Phase 1 readiness — a **real** gate as of the 2026-07-20 phase-1-ordering fix. Before that fix the init pump (`init_phase`) started only after `spawn_all_controllers` returned, so `wait_kinds_ready` always timed out (a dead 15 s per start / leader promotion) and provided no ordering; `prime` + phased `dispatch_init` now run the pump inline so the wait resolves promptly.

## LocalRawStore Maintenance

`FileSystemResourceController` creates a `LocalRawStore<K>` and maintains it along every event path. Etcd uses the same local-store type.

| Event / path | LocalRawStore operation |
|---|---|
| `InitApply` parsed successfully | `upsert(key, Arc::new(obj))` |
| `Apply` parsed successfully (init phase) | `upsert(key, Arc::new(obj))` |
| `Apply` parsed successfully (runtime phase, main loop) | `upsert(key, Arc::new(obj))` |
| `Delete` | `remove(&key)` |
| Worker: file exists and parsed successfully | `upsert(key, Arc::new(obj))` — ensure consistency with the version actually processed by the worker |
| Worker: file read fails | `remove(&key)` before the existing processed-cache revocation and inline `ParseError` status |
| Worker: file does not exist | `remove(&key)` — fallback in case a Delete event was lost (channel lagged) |

Maintaining via both the main loop and the worker is necessary: the main loop guarantees timely response, while the worker uses the freshest version re-read from disk to correct potential version drift, and also serves as a Delete fallback when the channel is lagged.

ARCH-096 keeps raw input consistent with the worker's existing read-failure
revocation policy. A read error (including invalid UTF-8 in an existing winner
file) removes the previous raw generation without adding a runtime-invalid
envelope. A repaired file can therefore recover through an ordinary requeue,
without an Apply event or manual envelope clearing. The mirrored worker test
checks RawStore and `get_raw_any` withdrawal, processed-cache revocation, persisted
`ParseError`, and subsequent raw/cache recovery with sidecar removal. RawStore
remains the canonical unfiltered input view; this does not change its consumers
or promise cross-kind atomicity.

## Differences from KubernetesCenter

| Aspect | FileSystemCenter | KubernetesCenter |
|--------|------------------|------------------|
| Event source | `notify::PollWatcher` (adaptive/overridden stat-diff) | K8s API Reflector watch stream |
| Status persistence | `.status` file (YAML) | K8s API `PATCH /status` (JSON Merge Patch) |
| Leader election | None (single instance, `leader_handle = None`) | Lease-based distributed election |
| Status-write guard | `is_none_or` always returns true | Guarded by `leader_handle.is_leader()` |
| 410 Gone handling | N/A | Triggers a full KubernetesController rebuild |
| File parsing | Extracts Kind/namespace/name from the file name | K8s API returns structured objects |
| RawStore implementation | `LocalRawStore` (manually maintained HashMap, also used by Etcd) | `KubeRawStore` (wraps the reflector Store) |
| Reload | Supported (triggered by Admin API, new server_id) | Supported (same as left) |
| Use cases | Development, testing, CI | Production K8s deployments |

## Multi-version Resource Compatibility

FileSystemCenter has no API Server to perform CRD multi-version conversion.
The current contract therefore accepts only the repository's canonical API
versions at the file-read boundary. Do not add legacy deserializers or
conversion shims for older YAML in this pre-release project.

- The watcher still dispatches by `kind`, not by `apiVersion`
- After `ResourceController` reads YAML, it must first identify the `apiVersion`
- Reject a noncanonical `apiVersion` with a clear validation error before writing
  to `LocalRawStore` or entering the workqueue.
- When the worker re-reads the file after dequeue, it must reuse the same
  canonical parse-and-validate logic.

Recommended principles:

- Each FileSystem controller maintains only one internal type
- Status persistence and downstream Gateway sync use the canonical type

## Key Files

- `edgion-controller/src/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter` (lifecycle)
- `edgion-controller/src/conf_mgr/conf_center/file_system/controller.rs` — `FileSystemController` (spawns resource controllers)
- `edgion-controller/src/conf_mgr/conf_center/file_system/storage.rs` — `FileSystemStorage` (CenterApi implementation)
- `edgion-controller/src/conf_mgr/conf_center/file_system/file_watcher.rs` — `FileSystemWatcher` (file watching + event dispatch)
- `edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs` — `FileSystemResourceController`
- `edgion-controller/src/conf_mgr/conf_center/file_system/status.rs` — `FileSystemStatusHandler` (.status file I/O)
- `edgion-controller/src/conf_mgr/conf_center/file_system/event.rs` — `FileSystemEvent` enum
- `edgion-controller/src/conf_mgr/conf_center/file_system/config.rs` — `FileSystemConfig`
