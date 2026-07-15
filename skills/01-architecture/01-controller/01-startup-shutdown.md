---
name: controller-startup-shutdown
description: edgion-controller startup and shutdown — CLI entry, initialization sequence, ShutdownHandle architecture, graceful shutdown.
---

# Controller Startup and Shutdown

## Entry Point

The binary entry point is `edgion-controller/src/bin/edgion_controller.rs`, which starts the `#[tokio::main(flavor = "multi_thread")]` multi-threaded runtime and performs two steps:

```text
EdgionControllerCli::parse_args()   // clap parses the command line
    └─► cli.run().await              // main flow
```

`EdgionControllerCli` is defined in `edgion-controller/src/cli/mod.rs` and flat-embeds `EdgionControllerConfig` via `#[derive(Parser)]`.

## Startup Sequence (`run()` method)

The startup path is split between `run()`, `run_after_logging()`, and
`start_services()`. The operational phases are:

| Step | Action | Description |
|------|--------|-------------|
| 1 | `EdgionControllerConfig::load()` | Merge CLI, environment variables, and config-file parameters |
| 2 | `init_controller_config()` / `init_global_test_mode()` | Initialize global config and test-mode flag |
| 3 | `init_environment()` | Initialize the working directory (work_dir priority: CLI > ENV > Config > default `.`), and the logging system |
| 4 | Observability initialization | Install metrics recorder and process-level observability |
| 5 | `ShutdownHandle::new()` | Create the shared signal broadcaster |
| 6 | `ConfMgr::create()` | Select FileSystem/Kubernetes/Etcd and create its `ControllerRuntime` |
| 7 | Security/auth initialization | Resolve admin/scoped tokens, build Authorizer, federation reload channel, and optional federation supervisor |
| 8 | Schema loading | Load CRD schemas in non-K8s mode; K8s delegates structural admission to the API server |
| 9 | Auto-CA pre-initialization | Prepare Kubernetes auto-CA rotation/identity inputs when enabled |
| 10 | Spawn ConfMgr lifecycle | Run backend watchers/leader lifecycle in a detached task |
| 11 | Validate conf_sync transport | `validate()` + `ensure_transport_configured()` fail startup before listeners when transport security is omitted/invalid |
| 12 | `start_services()` | Bind gRPC, Admin, probe, and metrics listeners; initialize TLS identity/cert watchers; block until shutdown |

The detached ConfMgr task logs lifecycle errors and leaves readiness false; such
an error does not by itself propagate as a non-zero process exit after listeners
have started. Operators must use `/ready` and the error metrics/logs.

### init_environment Details

- Working-directory priority: CLI `--work-dir` > env var `EDGION_WORK_DIR` > config-file field > default `.`
- Calls `init_work_dir()` to initialize and `validate()` to verify the directory layout
- Initializes the tracing logging system via `init_logging()` and returns a `WorkerGuard` (the guard must be held until the process exits, otherwise logs are lost)

### load_schemas Details

- **K8s mode**: skips loading and returns `SchemaValidator::empty()`; validation is delegated to the K8s API Server
- **Non-K8s mode**: loads CRD files from `{work_dir}/config/crd` to build a `SchemaValidator`; if loading fails or the count is 0, calls `process::exit(1)`

### start_services Details

Four listeners are involved:

```text
gRPC :12151       conf_sync (mTLS/explicit skip_tls)
Admin :12101      authenticated business API
Probe :12100      public health/readiness
Metrics :12190    public Prometheus scrape endpoint
```

Probe and metrics run in spawned listener tasks. Admin and gRPC are joined in the
main service future. The gRPC Provider dynamically obtains the latest
ConfigSyncServer after reload and returns `UNAVAILABLE` while it is absent.
Auto-CA can provision the in-memory server identity and start per-replica
identity sync; file-backed TLS starts a certificate watcher.

## ShutdownHandle Architecture

The shutdown mechanism is defined in `edgion-controller/src/conf_mgr/sync_runtime/shutdown.rs` and uses a `tokio::sync::watch` channel for broadcasting:

```text
ShutdownHandle
├── inner: Arc<ShutdownController>     // holds watch::Sender<bool>
└── signal: ShutdownSignal             // holds watch::Receiver<bool> (cloneable)
```

### Three Core Types

| Type | Role | Key methods |
|------|------|-------------|
| `ShutdownController` | Sender side, triggers shutdown | `new()` → `(Self, ShutdownSignal)`; `shutdown()` sends `true` |
| `ShutdownSignal` | Receiver side, cloneable and distributable to any component | `is_shutdown()` immediate check; `wait()` async wait |
| `ShutdownHandle` | Public-facing API wrapping the two above | `signal()` clones a receiver; `shutdown()` triggers; `wait_for_signals()` listens to OS signals |

### Signal Listening

`wait_for_signals()` runs in a spawned background task:

```text
tokio::select! {
    result = ctrl_c       => ...,   // tokio::signal::ctrl_c()  → SIGINT
    result = terminate    => ...,   // tokio::signal::unix::SignalKind::terminate() → SIGTERM
}
self.shutdown();   // broadcast true via the watch channel
```

On non-Unix platforms the SIGTERM branch is `std::future::pending()`, so only SIGINT is honored.

## Graceful Shutdown Flow

Shutdown is triggered either by an OS signal or by an internal `shutdown()` call. The signal is broadcast via the watch channel to every component holding a `ShutdownSignal`:

```text
SIGINT / SIGTERM
  │
  ▼
ShutdownHandle::wait_for_signals()
  │  self.shutdown() → watch::Sender sends true
  │
  ├─► Admin / probe / metrics drain immediately
  ├─► gRPC stream cancellation starts; listener drain is delayed 2 seconds
  └─► ConfMgr shutdown signal → stop watcher, ResourceController, worker
```

Shutdown order:

1. The OS signal is captured by `wait_for_signals()`
2. `watch::Sender` sends `true`; all `ShutdownSignal::wait()` calls return
3. Admin, probe, and metrics begin graceful shutdown; gRPC gets a two-second final-sync window
4. ConfMgr receives the signal and stops watcher/worker loops
5. The service join is bounded by the process shutdown budget (25 seconds after the signal); the process exits after completion or timeout

### Design Highlights

- **Single signal source**: the CLI creates a single `ShutdownHandle` and passes clones to ConfMgr, avoiding duplicate signal-handler registration
- **Broadcast semantics**: the `watch` channel allows any number of receivers; each component responds independently
- **Idempotent**: calling `shutdown()` multiple times is safe; `watch::Sender::send(true)` is idempotent
- **Logging guard**: `_log_guard` is held within the scope of `run()` to ensure all logs are flushed before shutdown

## Key Source Files

| File | Responsibility |
|------|----------------|
| `edgion-controller/src/bin/edgion_controller.rs` | Binary entry, tokio main |
| `edgion-controller/src/cli/mod.rs` | `EdgionControllerCli` definition, `parse_args()` / `run()` |
| `edgion-controller/src/conf_mgr/sync_runtime/shutdown.rs` | `ShutdownHandle` / `ShutdownController` / `ShutdownSignal` |
| `edgion-controller/src/conf_mgr/manager.rs` | `ConfMgr::create()` / `start_with_shutdown()` |
| `edgion-controller/src/api/` | Admin API (Axum) |
| `edgion-controller/src/conf_sync/` | gRPC ConfigSyncServer |
