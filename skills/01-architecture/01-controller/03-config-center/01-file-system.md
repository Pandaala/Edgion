---
name: config-center-file-system
description: FileSystemCenter implementation — local YAML directory watching, file naming convention, status persistence, startup scan, and runtime watcher.
---

# FileSystemCenter Implementation

FileSystemCenter is a ConfigCenter backend designed for development and testing. It watches a local YAML directory,
turning file changes into resource events, then runs them through the same Workqueue + ResourceProcessor pipeline used by KubernetesCenter.

## Architecture

```
FileSystemCenter
├── writer: FileSystemStorage       # CenterApi delegate (CRUD operations)
├── config: FileSystemConfig        # configuration (conf_dir, etc.)
├── config_sync_server: RwLock<Option<Arc<ConfigSyncServer>>>
├── reload_tx: Mutex<Option<mpsc::Sender<()>>>
└── controller_handle: Mutex<Option<JoinHandle<()>>>

FileSystemController.run()
├── FileSystemWatcher (shared, centralized file watching)
│   ├── [PollWatcher (1s stat-diff)] → [dispatcher] → [kind channels]
│   │                                        ├── HTTPRoute channel
│   │                                        ├── Gateway channel
│   │                                        └── ... other kind channels
│
├── spawn::<HTTPRoute, _>(HttpRouteHandler)
│   ├── ResourceProcessor + register into PROCESSOR_REGISTRY
│   └── FileSystemResourceController (subscribes to its kind channel)
│
├── spawn::<Gateway, _>(GatewayHandler)
│   └── ...
└── ~20 resource types, all running independently in parallel
```

## File Naming Convention

All resource files live in a single flat directory; resource metadata is encoded in the file name:

| Resource scope | File-name format | Example |
|----------------|------------------|---------|
| Namespaced | `{Kind}_{namespace}_{name}.yaml` | `HTTPRoute_default_my-route.yaml` |
| Cluster-scoped | `{Kind}__{name}.yaml` (double underscore) | `GatewayClass__edgion.yaml` |

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
- **Orphan cleanup**: at startup, `cleanup_orphans()` scans and deletes any `.status` files without a corresponding configuration file

## Startup Flow

The `start()` method of FileSystemCenter performs the following steps (with reload-loop support):

```
start(shutdown_handle)
└── loop:
    ├── 1. Create iteration_shutdown + reload channel + error channel
    ├── 2. FileSystemController.run()
    │   ├── Clean up orphan .status files
    │   ├── Phase 1: spawn base resource controllers
    │   │   (GatewayClass, Gateway, Secret, ReferenceGrant, Service, EndpointSlice)
    │   ├── wait_kinds_ready(phase1_kinds, 15s)
    │   ├── Phase 2: spawn dependent resource controllers
    │   │   (HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute, EdgionTls,
    │   │    BackendTLSPolicy, EdgionPlugins, EdgionStreamPlugins, PluginMetaData,
    │   │    LinkSys, EdgionAcme, EdgionGatewayConfig)
    │   └── FileSystemWatcher.run()
    │       ├── init_phase: scan directory → group by kind → Init/InitApply/InitDone
    │       └── runtime_phase: PollWatcher (1s stat diff) → Apply/Delete dispatched directly
    ├── 3. wait_registry_ready(30s)
    ├── 4. Trigger cross-namespace revalidation
    ├── 5. Create ConfigSyncServer + register all WatchObjs
    ├── 6. set_config_sync_server(Some)  ← gRPC service becomes available
    ├── 7. select! { shutdown / reload / error }
    └── 8. Cleanup: set_config_sync_server(None), abort controller, clear registry
        ├── Shutdown → return
        ├── Reload → continue loop (new server_id)
        └── Error → wait for shutdown signal then return
```

### Init Phase Details

`FileSystemWatcher.init_phase()` performs a synchronous directory scan:

1. `read_dir()` traverses the configuration directory
2. Filter `.yaml` / `.yml` files, exclude `.status` files
3. Read file contents and identify Kind via `ResourceKind::from_content()`
4. After grouping by kind, for each Kind send `Init` → `InitApply(path, content) * N` → `InitDone` in order
5. For Kinds that have subscribers but no files, still send `Init` → `InitDone` (so all controllers complete initialization)

### Runtime Phase Details

`FileSystemWatcher.runtime_phase()` uses the `PollWatcher` from the `notify` crate:

1. Create a `PollWatcher` (poll_interval = 1s) and forward raw events to `mpsc::unbounded_channel`
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

**Cost**: a fixed 1s latency (poll_interval). This is fully acceptable for a config controller.

**Lower bound on FS-mode latency**

In FileSystemCenter mode, the minimum latency from "writing a config file" to "Apply event dispatch" is 1 second (the next poll tick of PollWatcher). This is a property of PollWatcher's design, not a bug.

Tests and tools that assert on FS-mode response time (e.g. "status should update within < N seconds after writing the file") need to budget timeouts of at least 1.5 seconds.

**Known limitations**

- **Delete-then-immediately-recreate within the same 1s window is coalesced into a single Modify**: PollWatcher only sees snapshot diffs; the intermediate deleted state is invisible. If a ConfHandler relies on diff computation between on_delete / on_change (for example, to clean up reverse references), it must instead compare "old vs new value" inside `parse()` rather than relying on on_delete. See the implementations in `reference_grant.rs` and `backend_tls_policy.rs` for the pattern.
- The watcher only starts after init_phase has finished scanning, so events between the two are lost (this is a known separate issue, to be fixed independently).

## Phased Initialization

Like KubernetesCenter, FileSystemCenter uses a two-phase initialization:

| Phase | Resources | Description |
|-------|-----------|-------------|
| Phase 1 (base) | GatewayClass, Gateway, Secret, ReferenceGrant, Service, EndpointSlice | Other resources need to query these base resources during `parse()` |
| Phase 2 (dependent) | HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls, BackendTLSPolicy, EdgionPlugins, EdgionStreamPlugins, PluginMetaData, LinkSys, EdgionAcme, EdgionGatewayConfig | Started after Phase 1 completes |

Phase 2 waits for all Phase 1 kinds to be marked ready in `PROCESSOR_REGISTRY` (after a 15-second timeout, it falls back to parallel initialization).

## FsRawStore Maintenance

`FileSystemResourceController` creates an `FsRawStore<K>` in `new()` and injects it into the `processor`. It is then maintained synchronously along every event path:

| Event / path | FsRawStore operation |
|---|---|
| `InitApply` parsed successfully | `upsert(key, Arc::new(obj))` |
| `Apply` parsed successfully (init phase) | `upsert(key, Arc::new(obj))` |
| `Apply` parsed successfully (runtime phase, main loop) | `upsert(key, Arc::new(obj))` |
| `Delete` | `remove(&key)` |
| Worker: file exists and parsed successfully | `upsert(key, Arc::new(obj))` — ensure consistency with the version actually processed by the worker |
| Worker: file does not exist | `remove(&key)` — fallback in case a Delete event was lost (channel lagged) |

Maintaining via both the main loop and the worker is necessary: the main loop guarantees timely response, while the worker uses the freshest version re-read from disk to correct potential version drift, and also serves as a Delete fallback when the channel is lagged.

## Differences from KubernetesCenter

| Aspect | FileSystemCenter | KubernetesCenter |
|--------|------------------|------------------|
| Event source | `notify::PollWatcher` (1s stat-diff) | K8s API Reflector watch stream |
| Status persistence | `.status` file (YAML) | K8s API `PATCH /status` (JSON Merge Patch) |
| Leader election | None (single instance, `leader_handle = None`) | Lease-based distributed election |
| Status-write guard | `is_none_or` always returns true | Guarded by `leader_handle.is_leader()` |
| 410 Gone handling | N/A | Triggers a full KubernetesController rebuild |
| File parsing | Extracts Kind/namespace/name from the file name | K8s API returns structured objects |
| RawStore implementation | `FsRawStore` (manually maintained HashMap, dual-path maintenance) | `KubeRawStore` (wraps the reflector Store, automatically synced) |
| Reload | Supported (triggered by Admin API, new server_id) | Supported (same as left) |
| Use cases | Development, testing, CI | Production K8s deployments |

## Multi-version Resource Compatibility

FileSystemCenter has no API Server to perform CRD multi-version conversion, so multi-version compatibility must be handled at the file-read boundary:

- The watcher still dispatches by `kind`, not by `apiVersion`
- After `ResourceController` reads YAML, it must first identify the `apiVersion`
- For older-version resources (e.g. `BackendTLSPolicy/v1alpha3`), deserialize into the legacy type first
- Then immediately convert into the internal canonical type before writing to `FsRawStore`, entering the workqueue, and running the handler
- When the worker re-reads the file after dequeue, it must reuse the same "parse and normalize" logic

Recommended principles:

- Each FileSystem controller maintains only one internal type
- Older-version YAML is only an input compatibility layer
- Status persistence and downstream Gateway sync use the canonical type

## Key Files

- `src/core/controller/conf_mgr/conf_center/file_system/center.rs` — `FileSystemCenter` (lifecycle)
- `src/core/controller/conf_mgr/conf_center/file_system/controller.rs` — `FileSystemController` (spawns resource controllers)
- `src/core/controller/conf_mgr/conf_center/file_system/storage.rs` — `FileSystemStorage` (CenterApi implementation)
- `src/core/controller/conf_mgr/conf_center/file_system/file_watcher.rs` — `FileSystemWatcher` (file watching + event dispatch)
- `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs` — `FileSystemResourceController`
- `src/core/controller/conf_mgr/conf_center/file_system/status.rs` — `FileSystemStatusHandler` (.status file I/O)
- `src/core/controller/conf_mgr/conf_center/file_system/event.rs` — `FileSystemEvent` enum
- `src/core/controller/conf_mgr/conf_center/file_system/config.rs` — `FileSystemConfig`
