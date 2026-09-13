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
6. Configure outbound-egress defaults, Wasm OCI pull tuning, and the bounded process-memory artifact cache.
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

UDP listeners run on the auxiliary Tokio runtime, outside Pingora services. Their
registry owns cancellation tokens and serve-task handles. If listener construction
fails after any UDP listener has started, the CLI drains the registered UDP tasks
before dropping that runtime and returns the original startup error.

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
   diagnostic, cancels all UDP listeners, closes UDP registration, and sends the auxiliary
   shutdown signal. UDP continues serving during Pingora's own grace period; cancellation
   starts at the post-Pingora auxiliary drain, not at initial signal receipt.
4. Admin, probe, metrics, backend-cleaner, and other shutdown-aware tasks begin draining.
5. The background runtime joins UDP serve, cleanup, and upstream-receiver tasks within
   the same five-second auxiliary grace window. Completed child handles are also reaped
   during serving. Once producers have exited, remaining sessions submit terminal UDP
   access-log statistics and release session permits and sockets.
6. On grace timeout, remaining serve tasks are aborted together. Their owned child tasks
   are aborted on drop, and a task tracker acknowledges child destruction before runtime
   teardown. This acknowledgement is cooperative and may extend past five seconds; the
   window is not a hard process-exit deadline. Timeout or a failed UDP task is reported
   by a shutdown diagnostic and a non-zero exit. Repeated shutdown is idempotent.
7. After UDP producers finish, shared OTLP pipelines flush and shut down while the
   auxiliary runtime is still alive, including when UDP drain returned an error.
   The UDP result is preserved. Startup errors after conf-sync watches start also
   drain OTLP through the armed startup guard.
8. The runtime thread is joined before `EdgionGatewayCli::run()` completes. Internal
   policy-fatal errors retain their original non-zero outcome.

Terminal logs use the existing UDP logger. Cancellation may discard in-flight datagrams;
forced shutdown can abandon pending terminal logs. Submission does not establish durable
flush of the log sink: the shared [DataSender shutdown contract](../../../tasks/todo/data-sender-graceful-shutdown-contract.md)
remains separate. See the [UDP lifecycle regression guard](../../04-review/listeners/regression-guards/udp-process-shutdown.md)
for process-level wire coverage and its verification limits.

The background Tokio thread is therefore not an intentionally permanent `pending()` task.

Log delivery during process exit is best effort; the five-second window does not
acknowledge sender drain. See the accepted
[process-exit logging tradeoff](../../04-review/link-sys/intentional-design/process-exit-log-delivery-best-effort.md)
before proposing a shared sender shutdown contract.

## Listener hot-reload boundary

Listener sockets are bound once during Pingora construction. Runtime Gateway updates rebuild
listener metadata and per-listener route snapshots, but cannot add or remove Pingora services after
`Server::run(RunArgs)` begins.

- Adding a new listener port does not bind a socket in the running process.
- Removing a port does not close its already-bound socket.
- Route data and TLS certificates on existing sockets hot-reload through atomic snapshots.

Changing the listener port set requires a Gateway process restart/rolling restart.

Inbound PROXY protocol is part of the same socket boundary. After the baseline is frozen, a
change in PROXY socket semantics (Required / TrustedSources presence, CIDRs, or bind address of
an enabled policy) trips a process-wide monotonic fatal guard. Disabled-only listener keys may
appear or disappear without a restart: the already-bound socket keeps parsing no PROXY header.
Readiness becomes 503, every TCP service's accept filter rejects new connections before optional
stream plugins and before PROXY header parsing, and the internal shutdown watcher requests
FastShutdown. The process returns non-zero so its supervisor can restart it with the newest
complete valid snapshot. A revert cannot clear the fatal state. Contributor-only changes with
the same physical key and canonical declaration are diagnostic metadata and do not trip the
guard.

## Key source files

| File | Responsibility |
|------|----------------|
| `edgion-gateway/src/cli/mod.rs` | startup order, security gates, services, runtime handoff, shutdown |
| `edgion-gateway/src/cli/config.rs` | local config merge/defaults and management listener configuration |
| `edgion-gateway/src/cli/pingora.rs` | ServerConf, listener construction, blocking Pingora run loop |
| `edgion-gateway/src/runtime/server/listener_builder.rs` | protocol listener construction and backend capability checks |
| `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs` | conf-sync client identity hot reload |
