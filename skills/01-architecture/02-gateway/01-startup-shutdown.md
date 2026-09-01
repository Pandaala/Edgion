---
name: gateway-startup-shutdown
description: edgion-gateway startup, security gates, Pingora handoff, auxiliary-service shutdown, and listener hot-reload boundary.
---

# Gateway startup and shutdown

## Entry point

`EdgionGatewayCli::run()` creates a Tokio runtime, loads local configuration, performs async
initialization, constructs the Pingora server, moves the runtime to a shutdown-aware background
thread, and finally runs Pingora's blocking main loop.

The numbered comments in `edgion-gateway/src/cli/mod.rs` are the authoritative startup sequence.
Keep this page grouped by responsibility instead of freezing a shorter historical step count.

## Startup sequence

### 1. Local configuration and process globals

1. Load and merge `EdgionGatewayConfig`; initialize and validate the work directory.
2. Create the Tokio runtime and initialize system logging.
3. Reject `--integration-testing-mode` unless the explicit production-unsafe environment gate is set.
4. Install the Prometheus recorder before emitting startup metrics.
5. Detect host/module FIPS state, enforce `require_fips`, resolve `fips_policy`, and publish gauges.
6. Configure outbound-egress defaults and the Wasm OCI disk/pull cache.
7. Initialize rate-limit, LRU, sensitive-field, AllEndpointStatus, request-duration,
   backend-request, path-normalization, and access-log option globals.

`fips_policy` resolves environment over the already-merged CLI/file value over `auto`. An invalid
environment token warns and falls through; it never silently becomes `off`. See
[`04-tls/03-ssl-backends.md`](04-tls/03-ssl-backends.md).

### 2. Secure configuration sync

1. Validate `conf_sync_security`; omitted transport configuration fails closed.
2. If `auto_ca` is enabled, require configured certificate paths and retry initial client creation
   for the mounted Secret to appear (20 attempts, 2 seconds apart).
3. Create `ConfigSyncClient` with a 10-second connection timeout and publish the global
   `ConfigClient`.
4. Fetch Controller `ServerInfo`, initialize the test-mode globals, and start resource watches for
   the advertised kinds plus the server-metadata watch.
5. For an active file-backed mTLS identity, start the conf-sync certificate watcher so in-place
   Secret/cert renewal rebuilds the client channel without changing the active transport mode.

The config-independent security contract is documented with the sync subsystem; do not add a
plaintext example to this startup guide.

### 3. Auxiliary services and cache readiness

`start_auxiliary_services` starts:

- the LeastConn backend cleaner;
- health-check stores/manager initialization;
- the bearer-token-protected Admin API (`gateway.admin_listen`, default `127.0.0.1:12001`);
- the public probe server (`gateway.probe_listen`, default `0.0.0.0:12000`);
- the metrics server (`metrics.listen`, default `0.0.0.0:12090`).

All three HTTP servers use graceful-shutdown futures. If neither
`EDGION_GATEWAY_ADMIN_TOKEN` nor `gateway.admin_token` supplies a nonblank token, the Admin API
starts but fails closed with 503 on business routes.

Startup then waits until every watched `ClientCache` has completed its initial List. Once ready it
hydrates backend lists for services referenced by HTTP, gRPC, TCP, TLS, and UDP routes, and
initializes the access, SSL, TCP, TLS, and UDP loggers.

Before any Pingora service starts, the Gateway also performs typed inbound PROXY protocol
preflight over the complete Gateway snapshot. It rejects explicit invalid states, impossible Ready
states, noncanonical transported CIDRs, and conflicting declarations for one physical
`ListenerKey`. The validated key-to-declaration map is frozen for the process lifetime; only
`required` maps to Pingora's unbounded-source representation, while `trustedSources` always builds
an explicit matcher.

### 4. Pingora handoff

`create_and_configure_server()` runs in `spawn_blocking` while the Tokio runtime is active:

1. Build `ServerConf` (`threads`, `work_stealing`, grace periods, keepalive pool, error log).
2. Create and bootstrap `pingora_core::Server`.
3. Create `GatewayBase`, list Gateway resources, and configure startup listener sockets.

The Tokio runtime is then moved to a background thread that waits on the auxiliary shutdown watch.
The main thread calls `run_server()` → `Server::run(RunArgs)`. On Unix, `RunArgs` carries
`PolicyShutdownSignalWatch`, which races the ordinary `UnixShutdownSignalWatch` against the
process-wide inbound-PROXY-policy fatal watch. Unix SIGTERM/SIGINT keep their normal graceful/fast
semantics; the internal fatal branch returns `FastShutdown`. Non-Unix builds use default
`RunArgs`.

## Shutdown

1. Pingora handles SIGTERM/SIGINT and its configured grace period (default 30 seconds) plus
   graceful-shutdown timeout (default 10 seconds).
2. During process shutdown, the HTTP proxy disables keepalive and adds `Connection: close` where
   applicable.
3. After `Server::run(RunArgs)` returns, the main thread checks the bounded internal fatal
   diagnostic, sends the auxiliary shutdown signal, and returns an error only for that internal
   policy-fatal path.
4. Admin, probe, metrics, backend-cleaner, and other shutdown-aware tasks begin draining.
5. The background runtime allows a five-second drain window, returns, and is joined before
   `EdgionGatewayCli::run()` completes.

The background Tokio thread is therefore not an intentionally permanent `pending()` task.

## Listener hot-reload boundary

Listener sockets are bound once during Pingora construction. Runtime Gateway updates rebuild
listener metadata and per-listener route snapshots, but cannot add or remove Pingora services after
`Server::run(RunArgs)` begins.

- Adding a new listener port does not bind a socket in the running process.
- Removing a port does not close its already-bound socket.
- Route data and TLS certificates on existing sockets hot-reload through atomic snapshots.

Changing the listener port set requires a Gateway process restart/rolling restart.

Inbound PROXY protocol is part of the same socket boundary. After the baseline is frozen, any
effective key or declaration change trips a process-wide monotonic fatal guard. Readiness becomes
503, every TCP service's accept filter rejects new connections before optional stream plugins and
before PROXY header parsing, and the internal shutdown watcher requests FastShutdown. The process
returns non-zero so its supervisor can restart it with the newest complete valid snapshot. A
revert cannot clear the fatal state. Contributor-only changes with the same physical key and
canonical declaration are diagnostic metadata and do not trip the guard.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/cli/mod.rs` | startup order, security gates, services, runtime handoff, shutdown |
| `edgion-gateway/src/cli/config.rs` | local config merge/defaults and management listener configuration |
| `edgion-gateway/src/cli/pingora.rs` | ServerConf, listener construction, blocking Pingora run loop |
| `edgion-gateway/src/runtime/server/listener_builder.rs` | protocol listener construction and backend capability checks |
| `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs` | conf-sync client identity hot reload |
