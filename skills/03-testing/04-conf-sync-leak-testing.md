# Config-Sync Leak Detection Testing

> Verifies that all Gateway internal stores are properly cleaned up after configuration resources are deleted. Catches ConfHandler remove-path bugs, stale cache entries, and derived-store leaks.

## How It Works

```
1. Start environment (Controller + Gateway + test_server)
2. Load base configs (Gateway resource for leak-test listeners)
3. Capture baseline (store counts before any test injection)
4. For each cycle/scenario:
   a. Inject test YAML configs via edgion-ctl → Controller API
   b. Wait for gRPC sync → Gateway
   c. Verify configs arrived (ConfigClient cache counts > baseline)
   d. Delete injected configs
   e. Verify all stores return to baseline (ConfigClient + derived stores)
5. Stop environment
```

Verification checks **two layers**:
- **ConfigClient cache** — per-kind list counts via `GET /configclient/{kind}/list`
- **Derived stores** — aggregated stats via `GET /api/v1/debug/store-stats`

Baseline comparison (not absolute-zero) accounts for pre-existing resources from the test environment setup.

## Scripts

| Script | Purpose | Duration |
|--------|---------|----------|
| `run_conf_sync_test.sh` | Basic: N cycles of full inject → verify → delete → verify | ~25s (5 cycles) |
| `run_conf_sync_advanced_test.sh` | Advanced: 10 edge-case scenarios (orphan, wildcard, out-of-order, rapid-fire, etc.) | ~100s |

Both scripts are **self-contained** — they auto-start and auto-stop the environment via `start_all_with_conf.sh` / `kill_all.sh`.

### Running

```bash
# Basic test (full lifecycle: start → test → stop)
./examples/test/scripts/conf-sync/run_conf_sync_test.sh

# Advanced test
./examples/test/scripts/conf-sync/run_conf_sync_advanced_test.sh

# Attach to already-running environment (skip start/stop)
./examples/test/scripts/conf-sync/run_conf_sync_test.sh --no-start --keep-alive

# Stress test (50 cycles)
./examples/test/scripts/conf-sync/run_conf_sync_test.sh --stress
```

## Resource Coverage

| Resource | ConfigClient kind | Derived Store(s) |
|----------|-------------------|-------------------|
| HTTPRoute | `httproute` | RouteManager (exact_domains, wildcard_domains, catch_all, http_routes) |
| GRPCRoute | `grpcroute` | GrpcRouteManager (grpc_routes, resource_keys) |
| TCPRoute | `tcproute` | TcpRouteManager (routes_by_key, gateway_tcp_routes_map) |
| UDPRoute | `udproute` | UdpRouteManager (routes_by_key, gateway_udp_routes_map) |
| TLSRoute | `tlsroute` | GlobalTlsRouteManagers (route_cache, port_count) |
| EdgionTls | `edgiontls` | TlsStore (entries), TlsCertMatcher (port_count) |
| EdgionPlugins | `edgionplugins` | PluginStore (plugins) |
| EdgionStreamPlugins | `edgionstreamplugins` | StreamPluginStore (plugins) |
| BackendTLSPolicy | `backendtlspolicy` | BackendTLSPolicyStore (policies, reverse_index_targets) |
| Service | `service` | PolicyStore (total_services) |
| EndpointSlice | `endpointslice` | — (used by LB at runtime) |

## Advanced Scenarios

| # | Scenario | What it tests |
|---|----------|---------------|
| 1 | Orphan route | Route referencing non-existent Gateway + Service; `gateway-pending` path |
| 2 | Wildcard + catch-all | `wildcard_engine` rebuild + `catch_all_routes` cleanup |
| 3 | Delete Service before Route | Dangling backendRef; out-of-order resource removal |
| 4 | Delete Route before Service | LB policy cleanup (`batch_remove_routes`) |
| 5 | Rapid fire | 5x inject+delete without sync wait; CompressEvent coalescing |
| 6 | Duplicate apply | Same config applied 5x; tests idempotent add/update |
| 7 | Stream plugin | EdgionStreamPlugins + TCPRoute with `edgion.io/edgion-stream-plugins` annotation |
| 8 | BackendTLSPolicy | policies map + reverse_index full lifecycle |
| 9 | Mixed lifecycle | Add new configs while deleting old ones simultaneously |
| 10 | Full blast | All resource types injected and deleted at once |

## Debug API Endpoint

`GET /api/v1/debug/store-stats` (requires `--integration-testing-mode`)

Returns counts from all derived stores. Key fields:

```json
{
  "http_routes": { "exact_domains": 0, "wildcard_domains": 0, "has_catch_all": false, "http_routes": 0 },
  "grpc_routes": { "grpc_routes": 0, "resource_keys": 0 },
  "tcp_routes":  { "routes_by_key": 0 },
  "udp_routes":  { "routes_by_key": 0 },
  "tls_routes":  { "route_cache": 0 },
  "tls_store":   { "entries": 0 },
  "plugin_store": { "plugins": 0 },
  "stream_plugin_store": { "plugins": 0 },
  "backend_tls_policy": { "policies": 0, "reverse_index_targets": 0 },
  "policy_store": { "total_services": 0 }
}
```

## File Layout

```
examples/test/conf/conf-sync-leak-test/
├── base/
│   └── Gateway.yaml                      # Multi-protocol Gateway (HTTP/gRPC/TCP/UDP/TLS)
└── inject/
    ├── HTTPRoute_leak-http.yaml          # Exact domain route
    ├── HTTPRoute_leak-wildcard.yaml      # Wildcard domain (*.leak-wildcard.example.com)
    ├── HTTPRoute_leak-catchall.yaml      # No hostname (catch-all path)
    ├── HTTPRoute_leak-orphan.yaml        # Non-existent Gateway + Service refs
    ├── GRPCRoute_leak-grpc.yaml
    ├── TCPRoute_leak-tcp.yaml
    ├── TCPRoute_leak-tcp-sp.yaml         # With edgion.io/edgion-stream-plugins annotation
    ├── UDPRoute_leak-udp.yaml
    ├── TLSRoute_leak-tls.yaml
    ├── EdgionTls_leak-cert.yaml
    ├── EdgionPlugins_leak-plugins.yaml
    ├── EdgionStreamPlugins_leak-stream.yaml
    ├── BackendTLSPolicy_leak-btls.yaml
    ├── Service_leak-svc.yaml
    └── EndpointSlice_leak-svc.yaml

examples/test/scripts/conf-sync/
├── run_conf_sync_test.sh                 # Basic cyclic test
└── run_conf_sync_advanced_test.sh        # Advanced scenario test
```

## Adding New Resources to Coverage

1. Create YAML template in `conf-sync-leak-test/inject/`
2. Add the resource to `delete_injected_configs()` / `cleanup_all_test_resources()` in both scripts
3. Add the ConfigClient kind to the `kinds` array in `capture_baseline()`, `verify_configclient_empty()`, `verify_configs_present()`
4. If the resource has a dedicated derived store:
   - Add a `stats()` method to the store (return a `#[derive(serde::Serialize)]` struct)
   - Register it in `StoreStatsResponse` and `store_stats()` in `src/core/gateway/api/mod.rs`
   - Add the field path to `check_fields` in `verify_store_stats_empty()`
