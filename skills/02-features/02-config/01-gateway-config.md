---
name: gateway-config
description: Full schema for the edgion-gateway YAML configuration.
---

# Gateway YAML Configuration Schema

> Default file path: `config/edgion-gateway.yaml`, specified via `--config-file`.

## Files to Reference

- Sample configuration: `config/edgion-gateway.yaml`
- Configuration structure: `edgion-gateway/src/cli/config.rs`
- Startup logic: `edgion-gateway/src/cli/mod.rs`
- work_dir: `edgion-resources/src/work_dir.rs`
- Common log structures: `edgion-resources/src/observe.rs`, `edgion-resources/src/output.rs`

## Full Schema

```yaml
# Working directory
work_dir: "."

gateway:
  server_addrs: ["http://127.0.0.1:12151"]  # Required: Controller gRPC address
  # bind_ip: "0.0.0.0"                   # Local traffic socket IP; independent of Gateway.spec.addresses
  admin_listen: "127.0.0.1:12001"         # Admin API listen address (default: 127.0.0.1:12001, loopback-only)
  # probe_listen: "0.0.0.0:12000"         # Liveness/readiness probe port (default: 0.0.0.0:12000); binds 0.0.0.0 so kubelet can reach the pod IP
  # admin_token: "edg_..."                # Prefer EDGION_GATEWAY_ADMIN_TOKEN from a Secret
  # require_fips: false                    # Require a FIPS build at startup
  # fips_policy: auto                      # auto | "on" | "off"; strings only
  # outbound_egress_block_private: false   # Env can force on: EDGION_OUTBOUND_BLOCK_PRIVATE
  # outbound_egress_dns_cache_ttl_ms: 5000
  # outbound_http2_max_header_list_size: 16384 # 16KiB default; 16384..=65536; restart required
  # ─── Wasm `source.oci` memory cache + pull tuning (see `### gateway` below) ───
  # wasm_oci_cache_ttl_secs: 86400              # default 24h
  # wasm_oci_cache_purge_interval_secs: 60      # default 60s
  # wasm_oci_cache_max_bytes: 268435456         # default 256MiB; 0 disables retention
  # wasm_oci_cache_max_entries: 256            # default 256; 0 disables retention
  # wasm_oci_pull_timeout_secs: 60              # default
  # wasm_oci_pull_attempts: 3                   # default; includes the first attempt

# ─── gRPC channel security (REQUIRED — omitting it refuses to start) ───
# Pick exactly one; it must match the Controller's choice.
#
# (a) Kubernetes: auto_ca AND the cert paths. `auto_ca` alone is a startup failure on the
#     GATEWAY (auto_ca implies mTLS; the Controller-provisioned client cert must be readable
#     from disk). Only the Controller may set a bare `auto_ca: true`.
conf_sync_security:
  auto_ca: true
  active: primary
  certs:
    - name: primary
      cert: /etc/edgion/conf-sync-certs/client.crt
      key: /etc/edgion/conf-sync-certs/client.key
      ca: /etc/edgion/conf-sync-certs/ca.crt
#
# (b) file mode: real mTLS from operator-supplied certs
# conf_sync_security:
#   active: primary
#   certs:
#     - name: primary
#       cert: certs/grpc/client.crt    # Gateway client certificate
#       key: certs/grpc/client.key     # Client private key
#       ca: certs/grpc/ca.crt          # CA used to verify the Controller server
#
# (c) explicit plaintext opt-in (INSECURE; ERROR log + skip_tls metric)
# conf_sync_security:
#   active: insecure
#   certs:
#     - name: insecure
#       skip_tls: true

logging:
  # System log (not the business log)
  log_dir: logs
  log_prefix: edgion-gateway
  log_level: info                         # Module-level filtering supported: info,edgion::core::gateway::routes=debug
  json_format: false
  console: true
  buffer_size: 10000

The Gateway always caps `pingora_core::protocols::http::v1::client` and
`pingora_proxy::proxy_h1` at `error`, including when `RUST_LOG` requests a more verbose level.
Those upstream Pingora modules contain complete HTTP/1 request-header diagnostics, so their
security floor is not operator-overridable. Other targets retain normal module-level filtering.

server:
  threads: 0                               # 0 = CPU core count
  # downstream_tls_offload_threadpools: 2       # Optional, 1..=4
  # downstream_tls_offload_thread_per_pool: 2  # Optional, 1..=4; both fields required
  work_stealing: true                      # Tokio work-stealing
  grace_period_seconds: 30                 # Graceful shutdown wait
  graceful_shutdown_timeout_seconds: 10    # Shutdown timeout
  upstream_keepalive_pool_size: 128        # Upstream connection pool size
  # error_log: logs/pingora_error.log      # Pingora internal error log

# ─── URI path normalization policy ───
# This is Gateway process configuration, not EdgionGatewayConfig CRD state.
path_normalization:
  reject_escaped_slashes: false            # Reject encoded / or \\ (%2F / %5C)
  reject_invalid_percent_encoding: false   # Reject malformed or non-UTF-8 %XX

# ─── Business logs ───
# Each log type has its own enable switch and path; logging.log_dir does not affect these paths

access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # queueSize: 80000                   # Default: cpu_cores * 10000
      # rotation:
      #   strategy: daily                  # daily | hourly | never | { Size: 104857600 }
      #   maxFiles: 7
      #   checkIntervalSecs: 30

access_log_options:
  includeRawUri: false                    # Include the pre-normalization URI field

ssl_log:
  enabled: true
  output:
    localFile:
      path: logs/ssl.log

tcp_log:
  enabled: false
  output:
    localFile:
      path: logs/tcp_access.log

tls_log:
  enabled: true
  output:
    localFile:
      path: logs/tls_access.log

udp_log:
  enabled: false
  output:
    localFile:
      path: logs/udp_access.log

# ─── Global plugin configuration ───

ai:
  conversion:
    chat_to_anthropic:
      unary:
        max_input_size: 1MiB
        max_output_size: 1MiB
        hold_timeout: 120s
        memory_budget: 256MiB
        max_json_depth: 64
        max_json_nodes: 16384
        max_key_size: 256B
        max_source_events: 4096
        max_metadata_size: 64KiB

request_mirror:
  max_concurrent: 1024                   # Process-wide, 1..=65536
  max_buffered_bytes: 64MiB              # Retained mirror bodies, 1 byte..1GiB

rate_limit_local:
  default_estimator_slots_k: 64           # CMS default precision (× 64KB memory)
  max_estimator_slots_k: 1024             # CMS maximum precision cap
  gateway_instance_count: 1               # Static instance count (fallback when no Controller push)

tiny_ufo_ttl_cache:
  default_max_entries: 1024
  default_max_logical_bytes: 4MiB

lru_cache:
  default_capacity: 1024
  default_shards: 0                        # Retained independent LRU defaults

all_endpoint_status:
  max_endpoints: 20                        # Hard ceiling remains 50
  min_interval_ms: 1000

path_normalization:
  reject_escaped_slashes: false
  reject_invalid_percent_encoding: false

metrics:
  listen: "0.0.0.0:12090"
  genai_tokens:
    enabled: false                       # Optional bounded token rate/trend counter
  request_duration:
    enabled: false
    buckets: [1, 5, 10, 25, 50, 100, 200, 500, 1000, 2500, 5000, 10000]
    idle_timeout_secs: 600
    labels:
      route_name: true
      backend_name: true
      protocol: true
      status: true
  backend_requests:
    labels:
      gateway: true
      route: false
      backend: false
      protocol: true
      status: true

# ─── Log redaction (replace by default) ───

access_log_sensitive:
  mode: replace                    # off | replace | wrap
  # replace_with: "***"            # Replacement string when mode=replace
  # wrap_prefix: "<E>"             # Prefix when mode=wrap
  # wrap_suffix: "</E>"            # Suffix when mode=wrap

```

## Field Details

### path_normalization

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `reject_escaped_slashes` | `bool` | `false` | Reject paths containing encoded slash or backslash (`%2F` / `%5C`) instead of forwarding them upstream. |
| `reject_invalid_percent_encoding` | `bool` | `false` | Reject malformed hexadecimal escapes and percent-decoded non-UTF-8 input. |

The base normalization algorithm always runs. These flags only tighten rejection
behavior and are configured per Gateway process; there is no
`EdgionGatewayConfig.spec.pathNormalization` field.

### gateway

**Three-port model**: the gateway spawns three independent servers.

| Port | Default bind | Purpose |
|------|-------------|---------|
| Admin | `127.0.0.1:12001` | Admin API (`/configclient`, `/api/v1/server-info`) — loopback-only |
| Probe | `0.0.0.0:12000` | Liveness/readiness (`/health`, `/ready`) — outward-facing, kubelet probes pod IP |
| Metrics | `0.0.0.0:12090` | Prometheus metrics — independent listener, outward-facing |

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `server_addrs` | `Array<String>` | **Required** | Controller gRPC addresses. Single-address reconnect or multi-address candidate failover; see [Gateway-side recovery](../../01-architecture/03-controller-gateway-link/02-gateway-side.md). |
| `bind_ip` | `IP address` | `0.0.0.0` | Local traffic listener IP. Use an address owned by this Pod or host. `Gateway.spec.addresses` remains the external ingress address and does not determine this bind. CLI: `--bind-ip`. |

| `admin_listen` | `String` | `127.0.0.1:12001` | Admin API listen address. Defaults to loopback — sensitive endpoints not reachable cluster-wide. Remote access: `kubectl port-forward pod/<name> 12001:12001`. |
| `probe_listen` | `String` | `0.0.0.0:12000` | Probe listen address. Serves `/health` and `/ready` only. Binds all interfaces so kubelet can probe the pod IP directly. Not included in the gateway Service. |
| `admin_token` | `String?` | — | Shared Gateway Admin API bearer token. Config-file only; `EDGION_GATEWAY_ADMIN_TOKEN` takes precedence. Keep it in a Secret. No token means Admin business routes fail closed with 503. |
| `require_fips` | `bool` | `false` | Require the FIPS module; `EDGION_REQUIRE_FIPS` can force it on. |
| `fips_policy` | `auto \| on \| off` | `auto` | Downstream approved cipher/curve policy. Quote YAML `"on"`/`"off"`; booleans are invalid. Resolution: env → CLI → file → auto. |
| `outbound_egress_block_private` | `bool` | `false` | Reject private outbound URL targets by default; loopback/link-local remain blocked. Env can force it on. |
| `outbound_egress_dns_cache_ttl_ms` | `u64` | `5000` | Per-client outbound address memo TTL; env override: `EDGION_OUTBOUND_DNS_CACHE_TTL_MS`. |
| `outbound_http2_max_header_list_size` | `u32` | `16384` | Process-wide decoded HTTP/2 response-header-list limit for Gateway-initiated reqwest calls. Valid range: `16384..=65536`; changes require a restart. It does not affect Pingora route upstreams. |
| `wasm_oci_cache_ttl_secs` | `u64` | `86400` (24h) | Idle lifetime of a verified OCI memory-cache entry. Zero is clamped to 1s. |
| `wasm_oci_cache_purge_interval_secs` | `u64` | `60` | Background memory-cache sweep interval. Zero is clamped to 1s; it does not disable cleanup. |
| `wasm_oci_cache_max_bytes` | `u64` | `268435456` (256MiB) | Retained OCI payload allocation budget, including evicted allocations still borrowed by loaders. LRU eviction precedes best-effort admission; zero disables retention. This is not a process RSS or compiled VM limit. |
| `wasm_oci_cache_max_entries` | `usize` | `256` | Maximum retained OCI manifest/blob entries; at most 65,536. Zero disables retention. |
| `wasm_oci_pull_timeout_secs` | `u64` | `60` | Per-request timeout for OCI pull requests (manifest, token, and blob alike). Istio-agent `WASM_HTTP_REQUEST_TIMEOUT` parity. |
| `wasm_oci_pull_attempts` | `u32` | `3` | Total attempts (including the first) for each OCI pull request; transport errors and `5xx` retry, `4xx` never does. Istio-agent `WASM_HTTP_REQUEST_MAX_RETRIES` parity. |
| `geoip` | `GeoIpConfig` | *(empty)* | GeoIP database + background updater. **Sub-block of `gateway:`, snake_case, `deny_unknown_fields`.** See [`### geoip`](#geoip) below. |

`EDGION_GATEWAY_SCOPE=namespace/name` limits one Gateway process to one Gateway
and projects attached Routes and EdgionTls to that instance. Set
`EDGION_GATEWAY_UID=<Gateway UID>` for a Kubernetes dedicated instance so a
same-named replacement Gateway cannot reuse the old process.
Use it with a dedicated Deployment and Service for each Gateway that needs
independent traffic on a shared external port. The Service must carry
`edgion.io/gateway-service-ref: namespace/name` and
`edgion.io/gateway-service-uid: <Gateway UID>`. The Gateway must carry
`edgion.io/dedicated-gateway-instance: "true"`; without its dedicated Service,
it publishes no address and reports `Programmed=False`. The Controller
publishes the Service's assigned ClusterIP or all LoadBalancer ingress addresses.
`cicd/deploy/kubernetes/gateway/provision-instance.py namespace/name` creates
the dedicated Deployment and Service from the installed shared Deployment
template and marks the Gateway. Use `--service-type LoadBalancer` for a cloud
load balancer or NLB; configure provider-specific Service annotations on the
generated Service. If `Gateway.spec.addresses` requests a fixed address, it
must match the address actually assigned to that Service. Re-run the script
after listener edits; `--restart` rolls the data plane. For a deleted Gateway, use
`--delete --uid <old Gateway UID>` to remove that instance. Kubernetes does not
permit a cross-namespace ownerReference from a system Deployment to a Gateway
in another namespace, so cleanup is explicit in this manual deployment mode.
The manual script keeps generated resources in `edgion-system`; it does not
claim `GatewayInfrastructurePropagation` support or GEP-1762 infrastructure
layout compliance. For a physical deployment, set `gateway.bind_ip` for the
process default. A shared process can use different local IPs for same-port
Gateways through each Gateway's `edgion.io/local-bind-ip` annotation. Map each
external VIP or load balancer frontend to the matching local IP. A Gateway can
also be isolated in its own scoped process. Changing a local bind IP or the
listener set requires a process restart. Gateways that share one local socket
must use the same bind representation: leave `local-bind-ip` absent on all of
them, or set the same explicit IP on all of them. Mixing an inherited
`gateway.bind_ip` with an explicit annotation for that same IP creates
different logical listener keys and is rejected at startup.

### conf_sync_security

**Required section**: select a named `certs` entry (with optional `auto_ca`), or
select an explicit plaintext entry. Certificate paths make the Gateway connect
to the Controller's gRPC endpoint with mTLS.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `active` | `String?` | — | Name of the selected `certs` entry. Optional only when exactly one entry exists. |
| `certs` | `Vec<ConfSyncTlsConfig>` | `[]` | Named channel-security entries. |
| `auto_ca` | `bool` | `false` | Kubernetes only. The Controller provisions the Gateway client Secret; the Gateway must also configure the mounted cert paths in a selected entry. |

Each `certs` entry has:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | `String?` | — | Entry name; required when more than one entry exists. |
| `cert` | `String` | — | Client certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Client private key path (PEM) |
| `ca` | `String` | — | CA certificate path (used to verify the Controller server certificate) |
| `skip_tls` | `bool` | `false` | Explicit plaintext opt-in: `true` skips TLS and connects in plaintext (ERROR log + `skip_tls` metric). The ONLY way to run this channel in plaintext. |

> **Removal debt:** code still accepts the old singular
> `conf_sync_security.tls` field. It is not a supported configuration contract;
> do not use it in new configuration or examples.

> **`auto_ca` is asymmetric.** The Controller needs only `auto_ca: true` (it mints its own
> server identity in memory). The **Gateway** needs `auto_ca: true` **plus** a selected cert
> entry with certificate paths — a bare `auto_ca: true` exits non-zero (`edgion-gateway/src/cli/mod.rs`). The shipped
> `cicd/deploy/kubernetes/gateway/configmap.yaml` is the reference.
>
> **Omitting this section entirely makes the Gateway refuse to start** (non-zero exit) — it is
> not a plaintext default and it is not backward compatible with pre-2026-07-14 behavior.
> When mTLS is enabled, the URI scheme of `gateway.server_addrs` is automatically switched to `https://`.
> Generate certificates with `edgion-cli gen-certs --output file --output-dir <dir>`.

**`auto_ca` fail-close (data plane).** When `auto_ca: true` the gateway refuses to run the
conf_sync channel in plaintext, mirroring the Controller (whose provisioning failure aborts
startup):
> - `auto_ca: true` **with no `certs` entry at all** → permanent misconfiguration:
>   the gateway fails fast at startup (no plaintext, no retry). auto_ca implies mTLS, so the
>   cert paths must be set.
> - `auto_ca: true` **with cert paths but the Secret not yet provisioned** (cold start, the
>   mounted volume is `optional`) → the gateway does NOT downgrade to plaintext; it retries
>   client init within the startup-probe budget until the cert files appear, then connects mTLS.
> - `skip_tls: true` remains the only explicit plaintext escape hatch; it overrides `auto_ca`
>   but logs a high-visibility WARN.
>
> **Fail-close (2026-07-14, BREAKING):** an omitted `conf_sync_security` section — with or
> without `auto_ca` — makes the Gateway **refuse to start** (non-zero exit). This channel
> carries resolved Secrets, so plaintext must be an explicit choice. The startup error names
> the three ways forward: `auto_ca: true`, cert paths, or `skip_tls: true`.
>
> To use plaintext, opt in explicitly:
>
> ```yaml
> conf_sync_security:
>   active: insecure
>   certs:
>     - name: insecure
>       skip_tls: true   # INSECURE: plaintext conf_sync, ERROR log + skip_tls metric
> ```
>
> Otherwise configure `auto_ca: true` **together with the cert paths** (Kubernetes — a bare
> `auto_ca` is a second fail-close on the Gateway), or the cert paths alone (file mode). The
> retired `plaintext_default` system-error reason no longer exists; alert on
> `edgion_system_error_state{reason="skip_tls"}`.

### conf_sync client-cert hot reload (no new config fields)

The Gateway does not gain any new configuration surface from
`conf_sync_security.rotation` (that block is Controller-only — see the
[Controller config doc](./00-controller-config.md#conf_sync_securityrotation--conf_sync-auto-ca-cert-rotation)).
Hot reload watches the cert/key/ca paths in the selected
`conf_sync_security.certs` entry:

- **In-place file replacement is now hot-reloaded.** The Gateway watches the parent
  directories of the configured `cert`/`key`/`ca` paths (not the file inodes, since a
  Kubernetes Secret mount swaps files via an atomic `..data` symlink flip). A 2-second
  debounce coalesces the burst of filesystem events from that swap, so a rotated cert
  is picked up roughly ~2 seconds after the files land on disk.
- **Validate-before-swap.** On change, the new cert/key/ca are read and run through the
  same `validate_identity` check used at startup (leaf validity window, chain to CA, key
  match). On success, every consumer of the old gRPC channel is repointed at the new
  one. On failure, the current channel is kept untouched — a corrupt or mismatched
  write never severs the control-plane connection — and a
  `WARN` + `cert_reload_invalid` system-error mark is logged.
- **5-minute re-stat backstop.** Independent of the filesystem watcher, a periodic
  mtime/length re-stat catches inotify events dropped during a kubelet remount race
  (cheap, and removes the "watcher silently dead" failure mode). If the watcher itself
  cannot be established (construction failure, or an unwatchable cert directory), the
  watcher task does **not** exit: it marks the latching
  `edgion_system_error_state{reason="cert_watch_failed"}` gauge, logs an ERROR, and keeps
  running in backstop-only mode — reloads then land within 5 minutes instead of ~2 seconds.
  Ticket-level, not an outage; see the
  [Controller config alerting section](./00-controller-config.md).
- **The reload deliberately interrupts conf_sync streams for a few seconds — this is
  control-plane only, with zero data-plane impact.** Dropping the old gRPC channel does
  not by itself abort in-flight server-streaming responses, so a successful reload
  actively cancels a reload-generation token that the watch/meta loops select on; those
  loops break, reconnect on the newly-installed channel, and re-list. Expect a brief
  (seconds-long) gap in conf_sync resource sync — proxied traffic is unaffected.
- **Cold start in auto-CA mode has a bounded retry.** With configured paths but files
  not yet mounted, Gateway client TLS initialization retries 20 times at 2-second
  intervals (within the startup-probe budget). It never downgrades to plaintext. If the
  files remain unavailable after that window, startup fails. Non-auto-CA missing files
  fail without this provisioning retry.

### Admin API authentication

Gateway Admin auth is independent from Controller auth. Configure
`EDGION_GATEWAY_ADMIN_TOKEN` (recommended) or `gateway.admin_token`; blank means
unconfigured. The process still starts, but Admin business routes return 503. There is
no CLI token flag. See [03-auth-bootstrap.md](03-auth-bootstrap.md#gateway-providing-the-admin-token).

### metrics and process-global plugin limits

- `metrics.listen` controls the dedicated Prometheus listener. The request-duration
  histogram is disabled by default; enabling labels increases cardinality.
- `metrics.backend_requests.labels` controls labels on the always-on backend counter.
- `metrics.genai_tokens.enabled` controls the optional
  `edgion_gateway_genai_tokens_total` counter and defaults to `false`. The setting is
  captured once at Gateway startup, so changing it requires a restart. When disabled,
  the metric family is not registered. Its fixed labels produce at most twelve series:
  `direction` (`input` / `output`) × `provider` (`openai` /
  `openai_compatible` / `anthropic`) × `usage_source` (`reported` / `estimated`). It deliberately
  has no `model` label. This restart-reset counter is for Prometheus rate and trend
  aggregation, not persistent metering or billing. Per-request GenAI token usage is
  always written to the access log independently of this switch; configure the log
  backend to retain that ledger.
- `tiny_ufo_ttl_cache` supplies process-wide limits for positive OIDC
  introspection results, the only approved TinyUFO consumer. The
  logical byte setting is the per-cache-instance total weighting budget, not exact process
  RSS; Kubernetes/cgroup remains the hard process-memory boundary. Absolute
  safety ceilings are 65,536 cache entries, 4,194,304 TinyUFO weight units
  (256 MiB of 64-byte logical units). These ceilings are not defaults.
- `lru_cache` configures retained LRU users:
  OIDC access-token caching and the data-plane `ExternalName` DNS cache, whose
  capacity bound these defaults supply. The DNS cache takes the capacity but
  applies its own shard floor, so a `default_shards` below that floor does not
  lower its shard count. LDAP authentication is not cached.
- `all_endpoint_status` sets security ceilings for that plugin.
- `path_normalization` is process-local and restart-bound. It is not an
  `EdgionGatewayConfig` CRD field.

### server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `threads` | `usize` | CPU core count | Pingora worker thread count |
| `downstream_h2_stream_window_size` | `u32?` | `65535` | Startup HTTP/2 stream receive window in bytes |
| `downstream_h2_connection_window_size` | `u32?` | `65535` | Startup HTTP/2 connection receive window in bytes |
| `downstream_h2_max_concurrent_streams` | `u32?` | `100` | Startup HTTP/2 concurrent client streams per connection |
| `work_stealing` | `bool` | `true` | Tokio task stealing |
| `grace_period_seconds` | `u64` | `30` | Graceful shutdown wait time in seconds |
| `graceful_shutdown_timeout_seconds` | `u64` | `10` | Shutdown timeout in seconds |
| `upstream_keepalive_pool_size` | `usize` | `128` | Upstream keepalive connection pool size |
| `error_log` | `String?` | — | Pingora internal error log path |

> These settings belong exclusively to the local Gateway process, are resolved at startup, and
> require a restart to change. GatewayClass-scoped HTTP compression and downstream connection
> request limits belong to `EdgionGatewayConfig.spec.server`.

Downstream H2 fields apply uniformly to h2c and TLS H2. They are optional integer
values, with CLI-over-YAML precedence. `DownstreamH2Policy` validates the merged
config before startup services and builds from Pingora's `default_h2_options()`;
the same validated policy is installed in each normal HTTP proxy service before
TLS/TCP endpoints are added. See the
[operator contract and bounds](../../../docs/en/ops-guide/configuration/gateway-config.md#server)
and [regression guard](../../04-review/h2-grpc/regression-guards/downstream-h2-startup-policy.md).

### Business Log Schema

All business logs (access_log / ssl_log / tcp_log / tls_log / udp_log) share the same schema:

```yaml
LogConfig:
  enabled: bool                    # Whether enabled
  output:
    localFile:                     # The only output type currently
      path: String                 # Log file path (relative to work_dir)
      queueSize: usize?            # Write queue size (default: cpu_cores × 10000)
      rotation:                    # Rotation configuration (optional)
        strategy: RotationStrategy # daily | hourly | never | { Size: u64 }
        maxFiles: usize            # Number of files to retain (0 = unlimited)
        checkIntervalSecs: u64     # Rotation check interval (default 30s)
```

**RotationStrategy values**:
| Value | Description |
|-------|-------------|
| `"daily"` | Rotate at midnight daily |
| `"hourly"` | Rotate hourly |
| `"never"` | Do not rotate |
| `{ Size: 104857600 }` | Rotate by file size (bytes, default 100 MB) |

**Important**: `logging.log_dir` only affects the system log path; it does not affect business log paths. Business log paths are configured independently in each `output.localFile.path`.

### rate_limit

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_estimator_slots_k` | `usize` | `64` | CMS default slot count (×1K), memory ≈ slots_k × 64KB |
| `max_estimator_slots_k` | `usize` | `1024` | CMS maximum slot cap |
| `gateway_instance_count` | `u32` | `1` | Gateway instance count (denominator for cluster rate limiting; static fallback when there is no Controller) |

### request_mirror

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_concurrent` | `usize` | `1024` | Total active mirror tasks in this process; valid range `1..=65536`. |
| `max_buffered_bytes` | byte-size string | `64MiB` | Retained mirror request-body budget; valid range `1` byte through `1GiB`. |

This top-level Gateway YAML block is loaded at startup and requires a restart to
change. All route/plugin instances, including overlapping hot-reload generations,
share the budget. Invalid values and unknown keys fail startup; zero never means
unlimited. CLI merge preserves the file values; there are no dedicated CLI flags
or environment overrides. Exhaustion skips or cancels the mirror copy while the
main request continues. This budget does not bound total process RSS, request
headers, TLS, or connection overhead. Per-mirror tuning remains in HTTPRoute
`edgion.io/mirror-*` annotations or EdgionPlugins inline configuration; the
standard Gateway API filter and vendored CRDs are unchanged.

### ai.conversion.chat_to_anthropic.unary

These top-level Gateway process YAML fields apply only to selected non-streaming
OpenAI Chat-to-Anthropic conversion. They do not enable conversion or change native
AI, ordinary HTTP or SSE streaming. Omission uses the defaults below.

| Field | Default | Valid domain / meaning |
|-------|---------|------------------------|
| `max_input_size` | `1MiB` | 1 byte through `16MiB`; provider JSON bytes |
| `max_output_size` | `1MiB` | 1 byte through `16MiB`; serialized Chat completion bytes |
| `hold_timeout` | `120s` | `1ms` through `1h`; absolute ceiling from Hold activation, tightened by existing absolute deadlines |
| `memory_budget` | `256MiB` | Derived reservation `R` through `16GiB`; one process-wide admission pool |
| `max_json_depth` | `64` | Integer `1..=64` |
| `max_json_nodes` | `16384` | Integer `1..=16384`; includes unknown fields and overwritten duplicates |
| `max_key_size` | `256B` | 1 byte through `256B`; decoded JSON key bytes |
| `max_source_events` | `4096` | Integer `4..=65536`; source tasks, including empty body events |
| `max_metadata_size` | `64KiB` | `8KiB` through `64KiB`; cumulative retained head/trailer metadata, also at most 64 fields per projection |

Sizes and durations use the canonical byte-size and duration grammars. Startup
validates every field and computes the conservative reservation `R` with checked
arithmetic before configuration synchronization. Every new subtree (`ai`,
`conversion`, `chat_to_anthropic`, `unary`) rejects unknown keys. Invalid values,
zero, overflow or a pool smaller than `R` fail startup; the pool error reports the
required reservation bytes. There are no dedicated CLI flags or environment
aliases. All fields require a Gateway restart and remain shared across routes,
plugins and hot-reload generations.

At defaults, `R = 25MiB` and `floor(256MiB / R) = 10` concurrent reservations.
Changing advanced bounds changes `R`; it is not an operator override or an RSS
limit. Reservations cover conversion buffers/DOM/output and bounded retained
transport tasks/compact metadata. Original request/header storage, socket/TLS and
normal transport buffers, unrelated plugins, allocator overhead and fragmentation
are excluded. Allocation success is not guaranteed by reservation admission.
Both the exchange owner and all output clones must release before readmission.

Admission exhaustion or an active incompatible Hold claim returns fixed 503
before dispatch. Invalid successful provider JSON, unsupported media/encoding or
conversion/resource overflow returns fixed 502 before the final head/body commits;
Hold timeout returns fixed 504. True provider errors and selected plugin denials
retain their existing handling. Fully validated checked usage survives later
serialization/delivery failure; incomplete or invalid JSON has unknown usage and
fixed error bodies do not create character estimates.

See [the audited capacity ledger](../../../tasks/todo/ai-gateway-product-completion/task03-memory-ledger.md)
for representation assumptions and re-audit triggers.

### Upstream RemoteIp header

`EdgionGatewayConfig.spec.forwardedHeaders.remoteIpHeader` is the GatewayClass-
wide output-header name for the final HTTP `RemoteIp` sent to upstream services.
It defaults to `X-Real-IP` and may be customized, for example to
`X-Edgion-Remote-IP`. The Gateway validates the name, rejects reserved or
hop-by-hop headers (including `Host`, `Content-Length`, and
`X-Forwarded-For`), and overwrites the configured header after all request
plugins have completed. This is independent from
`spec.realIp.realIpHeader`: that field selects the downstream input header used
to resolve `RemoteIp`, while `remoteIpHeader` propagates the result upstream.

### access_log_sensitive

Controls the redaction behavior of fields marked as sensitive in access log, ssl.log, and plugin log. **`mode=replace` by default** (flipped from `off` — see rationale below): sensitive values render `***` out of the box.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `Enum` | `"replace"` | `"off"` / `"replace"` / `"wrap"` |
| `replace_with` | `String` | `"***"` | When `mode=replace`, replace the entire sensitive value |
| `wrap_prefix` | `String` | `"<E>"` | Prefix appended when `mode=wrap` |
| `wrap_suffix` | `String` | `"</E>"` | Suffix appended when `mode=wrap` |

**Behavior of the three modes** (using `direct_peer_ip=1.2.3.4` as an example):

| Mode | Output |
|------|--------|
| `off` | `"1.2.3.4"` |
| `replace` | `"***"` |
| `wrap` | `"<E>1.2.3.4</E>"` |

**Scope** — larger than the field list on this page alone; do not assume it stops at the built-ins:
- Access log fields: `direct_peer_ip` / `remote_ip` / `path` / `x_forwarded_for` / `sni` / `request_uri` / client certificate `subject` / `sans` / `cn`.
- `ext` fields declared via `AccessLogExtern` (`EdgionPlugins.spec.accessLogExtern`) render under this same switch: `routeLabel`/`routeAnnotation` sources are always clear, everything else follows `mode` unless whitelisted in `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys` (a whitelist **miss** still emits the key, rendered per `mode` — it does not drop the field). See [`../../03-coding/observability/00-access-log.md`](../../03-coding/observability/00-access-log.md).
- Plugin log: 46 `sensitive()`-marked parameter sites across 13 plugins (real_ip's IP, basic/key/ldap/jwt/hmac usernames, direct_endpoint's address, region_route's keys/regions/hosts, proxy_rewrite's method); user parameters of DSL `log_warn` / `log_error`.
- ssl.log: client certificate fields in mTLS allowlist validation failure errors.

**Why the default flipped to `replace`.** Because `ext` rendering follows this same switch, `mode=replace` by default is the only thing making `AccessLogExtern` "masked by default" true. **There is no configuration that gives readable `path` + masked `ext` cookie**: `off` makes the whitelist inert (everything clear, including `ext`); `wrap` puts the raw value into the local log file/stdout (tagged `<E>...</E>` so a downstream ES/ClickHouse pipeline can drop it, but the gateway's own file already has it in clear). This is a deliberate one-switch design, not an oversight — pick one of the two trade-offs. Note the interaction with `access_log_options.includeRawUri`: under the `replace` default, `request_uri` itself renders `***`, so enabling that toggle alone no longer yields a readable raw-entry view.

**Implementation details**:
- A global `AtomicU8` controls the mode, written once by `init_sensitive` at startup; immutable at runtime.
- When `off`, there is only a single atomic load overhead, effectively zero cost. This is no longer the default.
- Wrap mode uses a thread-local buffer with no heap allocation.
- For custom wrap prefixes/suffixes, the total length should not exceed 36 bytes; otherwise it reduces the per-entry writable length of the plugin log.
- Integration test fixtures set `access_log_sensitive.mode: off` explicitly in the baseline so assertions on `path` / `stage_logs` content stay readable; production defaults to `replace`.

### require_fips

Gateway-local (not cluster-synced) startup guard for BoringSSL FIPS builds. Also settable via
env `EDGION_REQUIRE_FIPS` (truthy: `1`/`true`/`yes`/`on`); the env takes effect the same as the
config field.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `require_fips` | `bool` | `false` | Require BoringSSL FIPS mode to be present at startup |

- `require_fips: false` (default): starts normally regardless of whether the binary is a FIPS
  build.
- `require_fips: true` on a FIPS build (image compiled with the `fips` cargo feature): the guard
  passes; the FIPS-approved default cipher/curve set applies automatically to downstream TLS.
- `require_fips: true` on a non-FIPS build (BoringSSL FIPS mode absent): the gateway logs a clear
  error and exits non-zero at startup (fail-close) — no silent downgrade.

This is a gateway-local field, not a cluster-synced one: a bad value can't be hot-pushed to
crashloop a fleet, and it surfaces on first run. The `fips` cargo feature itself is an image/build
concern, not something toggled at runtime. See
`skills/04-review/tls/regression-guards/boring-v5-fips-build-verification.md` for the current build contract. The
approved cipher/curve set and known limitations (TLS 1.3 ChaCha20 not restrictable in BoringSSL;
upstream/backend TLS ciphers are not FIPS-restricted).

Also note that `--require-fips` on the CLI **now takes effect**: `GatewayConfig::merge()` was
previously missing this field, so a CLI flag was parsed by clap and then silently discarded by the
config merge, leaving only YAML/env able to set it. `require_fips` keeps its OR semantics in the
merge — the CLI flag can only turn it on, never override a file's `true` back to `false`.

### fips_policy

Gateway-local tri-state that selects the FIPS-approved TLS policy (approved cipher list + curves)
on downstream listeners, independent of whether the binary is a FIPS build.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `fips_policy` | `"auto" \| "on" \| "off"` | `auto` | Select the FIPS-approved cipher/curve enforcement regardless of BoringSSL FIPS mode |

- `auto` (default): follow the host kernel. Enforces the approved set when the host kernel reports
  FIPS mode via `/proc/sys/crypto/fips_enabled`; otherwise does not. This is cipher-suite policy
  alignment only — it is **not** a compliance claim.
- `on`: always enforces the approved cipher/curve set, even on a normal (non-FIPS) boringssl build.
  Two uses: (1) exercise the FIPS policy in integration tests without a FIPS toolchain/module (the
  approved-suite behavior is pure SSL config, so it works on any boringssl build); (2) opt into the
  approved-suite restriction as defense-in-depth.
- `off`: never force-enables the policy. On a genuine FIPS build the approved profile still applies
  unconditionally — `off` can only disable the module-less (forced) half of the gate, never real
  FIPS-build enforcement. Effective gate is `fips_module_available() OR fips_policy_forced` where
  `fips_policy_forced` is derived from this setting (and, for `auto`, from the host state).
  Operator `EdgionTls.ciphers` still overrides the cipher list (approved curves still apply). This
  field does **not** provide BoringSSL FIPS mode. A FIPS-mode build is necessary for this posture,
  but does not by itself establish certificate scope, provenance, or deployment compliance.

**Accepted values.** Only the three strings above — no legacy booleans, no legacy truthy/falsey
tokens. A YAML boolean (`fips_policy: true`) is a hard deserialize error. YAML 1.1 reads a bare
`on` / `off` scalar as a boolean, so those two values must be quoted: `fips_policy: "on"` /
`fips_policy: "off"`; `auto` needs no quoting.

**Resolution order, highest first: env → CLI → config file → `auto`.**

### geoip

Gateway-local (not cluster-synced — the CRD has multiple named instances, but the database mmap
and updater task are one per process) settings backing the `GeoIpLocation` plugin, registered
into both `EdgionPlugin` (HTTP) and `EdgionStreamPlugin` (L4 Stage-1 only) with one shared config
shape (`GeoIpLocationConfig` — `defaultAction`/`failOpen`/`denyRuleGroups`/`allowRuleGroups`, see
`13-edgion-plugins.md`/`14-edgion-stream-plugins.md`). Struct:
`edgion_gateway::geoip::GeoIpConfig` (`edgion-gateway/src/geoip/config.rs`). Full field reference
and every operator pitfall (the `gateway:`-nesting trap, SIGBUS-on-in-place-write, update-volume
sizing (`max_db_bytes` + 2× current database size), absent-dimension rule-miss behavior,
`failOpen` semantics, `RemoteIp` spoofability, ...): `docs/en/ops-guide/gateway/geoip.md`.

**Must be nested under `gateway:`.** `GatewayConfig` uses `#[command(flatten)]` for the CLI but
NOT `#[serde(flatten)]` for YAML, so a top-level `geoip:` key is ignored by the outer struct and
leaves `GeoIpConfig` at its empty defaults. The Gateway continues starting but emits a structured
startup ERROR log with `field_path="geoip"` and `action="ignored"`; it never logs the value or raw
YAML. `GeoIpConfig`'s own `deny_unknown_fields` remains stricter and rejects a typo *inside* the
block (for example `dbPath`).

There is no master-switch field: an unset `db_path` means no database (geo lookups report
`Unavailable`), and there is no admin endpoint to introspect the loaded state — use
`edgion_geoip_db_loaded` / `edgion_geoip_db_build_epoch_seconds` (metrics) and the access log's
`geo` block (per-request lookup outcome) instead.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `db_path` | `PathBuf?` | none | Local MMDB path. May not exist at first start (no bundled image database) — normal, not a failure. |
| `update_url` | `String?` | none | Update source; empty/absent never spawns the updater. Redacted in logs. |
| `update_auth_header_env` | `String?` | none | Env var name whose value is validated as an HTTP `Authorization` header. Invalid values are rejected without being echoed. The typed value is marked sensitive before storage: clones retain cooperative `Debug` masking and HTTP/2 never-indexed metadata. This is not encryption, memory zeroization, or blanket HTTP/1 wire-log redaction. |
| `update_interval_sec` | `u64` | `86400` | Steady-state cycle interval, after the first successful load. Must be greater than zero when `update_url` is set. |
| `retry_interval_sec` | `u64` | `600` | Steady-state retry backoff; not used before the first success (short escalating backoff instead, capped at 30s). Must be greater than zero when `update_url` is set. |
| `connect_timeout_sec` | `u64` | `10` | Connect timeout. Must be greater than zero when `update_url` is set. |
| `download_deadline_sec` | `u64` | `600` | Whole-cycle deadline. `reqwest` client-level `total_timeout` stays unset on purpose. Must be greater than zero when `update_url` is set. |
| `stall_timeout_sec` | `u64` | `60` | No-progress abort. Must be greater than zero when `update_url` is set. |
| `max_db_bytes` | `u64` | `268435456` | Hard cap enforced while streaming. Must be greater than zero when `update_url` is set. |

**Never write `db_path` in place** (`kubectl cp`, initContainer overwrite, truncate+rewrite) —
the gateway holds it memory-mapped, so an external in-place write causes SIGBUS (not a Rust
panic, not caught by `catch_unwind`, kills the process mid-request). Only the built-in updater,
or an external writer using temp-file-on-same-filesystem + `rename(2)`, are supported. Size the
update volume for `max_db_bytes + 2 × current database size`; when no database exists yet, the
requirement is `max_db_bytes` alone.

Persisted ETag/Last-Modified validators are eligible only while `db_path` is a regular file and
the provider has an active database. If either half is unavailable, the updater clears both the
in-memory validator and the sidecar before making an unconditional request. A 304 without both a
sent request validator and that usable local state is a `fetch` failure: validators are cleared,
the updater enters cold backoff, and the next request is unconditional.

- **Env** (`EDGION_FIPS_POLICY`): accepts only `auto`/`on`/`off` (case-insensitive, trimmed).
  Unset or whitespace-only falls through to CLI/file. Any other token — including the old
  `1`/`true`/`0`/`false` — is unrecognized: the gateway **WARNs and ignores it**, falling through
  to CLI/file rather than treating it as `off`, so a typo can never silently disable enforcement.
- **CLI** (`--fips-policy`): `value_enum` with `num_args = 0..=1` and `default_missing_value = "on"`,
  so a bare `--fips-policy` means `on`, `--fips-policy=off` / `--fips-policy=auto` also work, and
  omitting the flag leaves the field `None` (not `Some(Auto)`) so it does not clobber a config
  file's value during merge. `--fips-policy` **now takes effect**: `GatewayConfig::merge()` was
  previously missing this field entirely, so the CLI value was parsed and then silently discarded;
  only YAML/env reached the runtime.
- **Config file**: the YAML value, if the field is present.
- **`auto`**: the default if none of the above set a value.

## `integration_testing_mode`

This is a CLI flag, not a YAML config section. It is **production-unsafe** and is
guarded by a second, explicit gate: the flag is refused at startup (fail-close)
unless the `EDGION_ALLOW_INTEGRATION_TESTING_MODE` environment variable is also
set to a truthy value (`1` / `true` / `yes` / `on`). A single accidental CLI flag
in a production rollout therefore aborts startup loudly instead of silently
widening the surface.

```bash
EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
  ./target/debug/edgion-gateway --config-file config/edgion-gateway.yaml --integration-testing-mode
```

When enabled, it additionally activates:
- Access Log Store
- Test-only metrics tags (per-request `test_data` labels — unbounded cardinality)
- `/api/v1/testing/*` debug endpoints
- Loopback-upstream allowance for endpoint-derived backends

While active it also raises `edgion_system_error_state{component="gateway_runtime",
reason="integration_testing_mode"} = 1`, so monitoring catches accidental
production enablement even when the startup WARN goes unread.

## Most Common Gateway Configuration Changes in This Project

- Change the Controller gRPC address: change `gateway.server_addrs`
- Change system log noise: change `logging.log_level`
- Toggle business logs: change `access_log` / `ssl_log` / `tcp_log` / `udp_log`
- Tune Pingora workers and keepalive: change `server`
- Tune the default RateLimitLocal precision: change `rate_limit`
- Require BoringSSL FIPS mode at startup (FIPS builds only): change `require_fips` or set `EDGION_REQUIRE_FIPS`
- Tune the Wasm `source.oci` memory cache / pull behavior: change `gateway.wasm_oci_cache_max_entries` / `wasm_oci_cache_ttl_secs` / `wasm_oci_cache_purge_interval_secs` / `wasm_oci_cache_max_bytes` / `wasm_oci_pull_timeout_secs` / `wasm_oci_pull_attempts`. Artifacts are not persisted across restart. Up to four OCI acquisition pipelines bound transient work separately from retained cache allocations. Last-plugin removal clears retained entries; active compiled modules follow their existing reference lifecycle.

## OTLP output references

Async `access_log`, `tcp_log`, `tls_log` and `udp_log` primary/fallback outputs
accept `otlp: {linkSysRef: {namespace: observability, name: collector}}`.
Endpoint, timeout, authentication and TLS belong in `LinkSys.spec.config` with
`type: otlp`, never in process YAML. There is no legacy inline OTLP form.
All LinkSysRef values validate Kubernetes namespace/name syntax and length while
loading process YAML, including disabled blocks. Invalid values produce fixed
errors before sender identity construction; valid unresolved references initialize
unhealthy and participate in first-healthy fallback. SSL sync logs remain file-only.
