---
name: controller-startup-shutdown
description: edgion-controller startup and shutdown — CLI entry, initialization sequence, ShutdownHandle architecture, graceful shutdown.
---

# Controller Startup and Shutdown

## Entry Point

The binary entry point is `src/bin/edgion_controller.rs`, which starts the `#[tokio::main(flavor = "multi_thread")]` multi-threaded runtime and performs two steps:

```text
EdgionControllerCli::parse_args()   // clap parses the command line
    └─► cli.run().await              // main flow
```

`EdgionControllerCli` is defined in `src/core/controller/cli/mod.rs` and flat-embeds `EdgionControllerConfig` via `#[derive(Parser)]`.

## Startup Sequence (`run()` method)

`run()` executes the steps below in order; each step depends on the previous one finishing:

| Step | Action | Description |
|------|--------|-------------|
| 1 | `EdgionControllerConfig::load()` | Merge CLI, environment variables, and config-file parameters |
| 2 | `init_controller_config()` / `init_global_test_mode()` | Initialize global config and test-mode flag |
| 3 | `init_environment()` | Initialize the working directory (work_dir priority: CLI > ENV > Config > default `.`), and the logging system |
| 4 | `ShutdownHandle::new()` | Create the shutdown handle and spawn a background task to wait for SIGINT/SIGTERM |
| 5 | `ConfMgr::create()` | Create the configuration manager (ConfigSyncServer is None at this point) |
| 6 | spawn `ConfMgr::start_with_shutdown()` | Start the ConfMgr lifecycle in the background (leader election, watcher, link) |
| 7 | `load_schemas()` | Load CRD schemas (only in non-K8s mode; in K8s mode the API Server validates) |
| 8 | `start_services()` | Start gRPC + Admin API; blocks until shutdown |

### init_environment Details

- Working-directory priority: CLI `--work-dir` > env var `EDGION_WORK_DIR` > config-file field > default `.`
- Calls `init_work_dir()` to initialize and `validate()` to verify the directory layout
- Initializes the tracing logging system via `init_logging()` and returns a `WorkerGuard` (the guard must be held until the process exits, otherwise logs are lost)

### load_schemas Details

- **K8s mode**: skips loading and returns `SchemaValidator::empty()`; validation is delegated to the K8s API Server
- **Non-K8s mode**: loads CRD files from `{work_dir}/config/crd` to build a `SchemaValidator`; if loading fails or the count is 0, calls `process::exit(1)`

### start_services Details

gRPC and Admin API run concurrently via `tokio::join!`, each holding its own `ShutdownSignal`:

```text
tokio::join!(
    grpc_server.serve_with_shutdown(grpc_addr, grpc_shutdown),
    serve_admin_api_with_shutdown(conf_mgr, schema_validator, admin_addr, admin_shutdown),
)
```

The gRPC server uses a Provider pattern (`ConfMgrProvider`) to dynamically obtain the latest `ConfigSyncServer` instance, allowing reload without restarting the service. Before `ConfigSyncServer` is ready, gRPC returns `UNAVAILABLE`.

## ShutdownHandle Architecture

The shutdown mechanism is defined in `src/core/controller/conf_mgr/sync_runtime/shutdown.rs` and uses a `tokio::sync::watch` channel for broadcasting:

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
  ├─► gRPC server shutdown signal → tonic graceful_shutdown
  ├─► Admin API shutdown signal → axum graceful_shutdown
  └─► ConfMgr shutdown signal → stop watcher, ResourceController, worker
```

Shutdown order:

1. The OS signal is captured by `wait_for_signals()`
2. `watch::Sender` sends `true`; all `ShutdownSignal::wait()` calls return
3. The shutdown futures of gRPC and Admin API complete; `tokio::join!` unblocks
4. ConfMgr receives the signal and stops watcher and worker loops
5. `run()` returns and `main()` exits

### Design Highlights

- **Single signal source**: the CLI creates a single `ShutdownHandle` and passes clones to ConfMgr, avoiding duplicate signal-handler registration
- **Broadcast semantics**: the `watch` channel allows any number of receivers; each component responds independently
- **Idempotent**: calling `shutdown()` multiple times is safe; `watch::Sender::send(true)` is idempotent
- **Logging guard**: `_log_guard` is held within the scope of `run()` to ensure all logs are flushed before shutdown

## Key Source Files

| File | Responsibility |
|------|----------------|
| `src/bin/edgion_controller.rs` | Binary entry, tokio main |
| `src/core/controller/cli/mod.rs` | `EdgionControllerCli` definition, `parse_args()` / `run()` |
| `src/core/controller/conf_mgr/sync_runtime/shutdown.rs` | `ShutdownHandle` / `ShutdownController` / `ShutdownSignal` |
| `src/core/controller/conf_mgr/manager.rs` | `ConfMgr::create()` / `start_with_shutdown()` |
| `src/core/controller/api/` | Admin API (Axum) |
| `src/core/controller/conf_sync/` | gRPC ConfigSyncServer |
