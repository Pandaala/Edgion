---
name: grpc-watch-none-arm-silent-break
description: Use when a reviewer flags that the `grpc_server.rs` / `config_sync_server.rs` watch-pipeline `None => break` arms are silent when the upstream channel drops (Gateway sees indistinguishable EOF, tonic #1544) — fixed; both middle layers now log a warn before breaking.
---

# gRPC watch pipeline `None => break` arms now log before breaking (Fix 04)

**Status / verdict**: fixed (Fix 04)

Finding form: "`grpc_server.rs` / `config_sync_server.rs` `None => break` arms are silent when upstream channel drops — Gateway sees indistinguishable EOF (tonic #1544)".

Fixed. The two middle-layer watch-pipeline arms now emit a `tracing::warn!` before breaking:

- `grpc_server.rs:277` now emits `tracing::warn!(component="grpc_server", kind, client_id, ...)`.
- `config_sync_server.rs:229` now emits `tracing::warn!(component="config_sync_server", server_id, ...)`.

The bottom layer (`cache.rs:325`) retains its pre-existing `tracing::debug!`. With these two additions the three-layer observability chain (cache → config_sync_server → grpc_server) is now complete, so an upstream channel drop is distinguishable from a normal EOF in the logs.

Note on expected firing: the `None` arm may also fire during a deliberate `reset_for_relink()` → `clear_all()` teardown. This is expected, and the `warn!` still carries useful diagnostic value (it records which client/server stream ended and why), so it is intentionally not downgraded for the teardown path.

Re-reports on the two middle-layer silence are not accepted. Before re-reporting, verify the current `grpc_server.rs` and `config_sync_server.rs` `None => break` arms do not already carry the `tracing::warn!` calls described above.
