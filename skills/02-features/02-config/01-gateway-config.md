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
  server_addr: "http://127.0.0.1:12151"   # Required: Controller gRPC address
  admin_listen: "127.0.0.1:12001"         # Admin API listen address (default: 127.0.0.1:12001, loopback-only)
  # probe_listen: "0.0.0.0:12000"         # Liveness/readiness probe port (default: 0.0.0.0:12000); binds 0.0.0.0 so kubelet can reach the pod IP
  # admin_token: "edg_..."                # Prefer EDGION_GATEWAY_ADMIN_TOKEN from a Secret
  # require_fips: false                    # Require a FIPS build at startup
  # fips_policy: auto                      # auto | "on" | "off"; strings only
  # outbound_egress_block_private: false   # Env can force on: EDGION_OUTBOUND_BLOCK_PRIVATE
  # outbound_egress_dns_cache_ttl_ms: 5000
  # ─── Wasm `source.oci` disk cache + pull tuning (see `### gateway` below) ───
  # wasm_oci_cache_dir: wasm-oci-cache          # default; "" disables disk caching
  # wasm_oci_cache_ttl_secs: 86400              # default 24h
  # wasm_oci_cache_purge_interval_secs: 3600    # default 1h
  # wasm_oci_cache_max_bytes: 536870912         # default 512MiB; 0 = uncapped
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

server:
  threads: 0                               # 0 = CPU core count
  work_stealing: true                      # Tokio work-stealing
  grace_period_seconds: 30                 # Graceful shutdown wait
  graceful_shutdown_timeout_seconds: 10    # Shutdown timeout
  upstream_keepalive_pool_size: 128        # Upstream connection pool size
  downstream_keepalive_request_limit: 1000 # Maximum requests per downstream connection (0 = unlimited)
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
      # queue_size: 80000                  # Default: cpu_cores * 10000
      # rotation:
      #   strategy: daily                  # daily | hourly | never | { Size: 104857600 }
      #   max_files: 7
      #   check_interval_secs: 30

access_log_options:
  include_raw_uri: false                  # Include the pre-normalization URI field

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

rate_limit:
  default_estimator_slots_k: 64           # CMS default precision (× 64KB memory)
  max_estimator_slots_k: 1024             # CMS maximum precision cap
  gateway_instance_count: 1               # Static instance count (fallback when no Controller push)

lru_cache:
  default_capacity: 1024
  default_shards: 0                        # 0 = unsharded

all_endpoint_status:
  max_endpoints: 20                        # Hard ceiling remains 50
  min_interval_ms: 1000

path_normalization:
  reject_escaped_slashes: false
  reject_invalid_percent_encoding: false

metrics:
  listen: "0.0.0.0:12090"
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

# ─── Log redaction (off by default) ───

access_log_sensitive:
  mode: off                        # off | replace | wrap
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
| `server_addr` | `String` | **Required** | Controller gRPC address |
| `admin_listen` | `String` | `127.0.0.1:12001` | Admin API listen address. Defaults to loopback — sensitive endpoints not reachable cluster-wide. Remote access: `kubectl port-forward pod/<name> 12001:12001`. |
| `probe_listen` | `String` | `0.0.0.0:12000` | Probe listen address. Serves `/health` and `/ready` only. Binds all interfaces so kubelet can probe the pod IP directly. Not included in the gateway Service. |
| `admin_token` | `String?` | — | Shared Gateway Admin API bearer token. Config-file only; `EDGION_GATEWAY_ADMIN_TOKEN` takes precedence. Keep it in a Secret. No token means Admin business routes fail closed with 503. |
| `require_fips` | `bool` | `false` | Require the FIPS module; `EDGION_REQUIRE_FIPS` can force it on. |
| `fips_policy` | `auto \| on \| off` | `auto` | Downstream approved cipher/curve policy. Quote YAML `"on"`/`"off"`; booleans are invalid. Resolution: env → CLI → file → auto. |
| `outbound_egress_block_private` | `bool` | `false` | Reject private outbound URL targets by default; loopback/link-local remain blocked. Env can force it on. |
| `outbound_egress_dns_cache_ttl_ms` | `u64` | `5000` | Per-client outbound address memo TTL; env override: `EDGION_OUTBOUND_DNS_CACHE_TTL_MS`. |
| `wasm_oci_cache_dir` | `String?` | `wasm-oci-cache` | Directory for the Wasm `source.oci` disk cache (manifests + layer blobs, content-addressed). Relative paths resolve against `work_dir`. **An empty string (`""`) disables disk caching.** |
| `wasm_oci_cache_ttl_secs` | `u64` | `86400` (24h) | Seconds since a cached OCI file was last used before it is eligible for purge. |
| `wasm_oci_cache_purge_interval_secs` | `u64` | `3600` (1h) | Interval between purge sweeps of the OCI disk cache. |
| `wasm_oci_cache_max_bytes` | `u64` | `536870912` (512MiB) | Total-size budget for the OCI disk cache, enforced on every write with LRU eviction. **`0` removes the cap (uncapped).** |
| `wasm_oci_pull_timeout_secs` | `u64` | `60` | Per-request timeout for OCI pull requests (manifest, token, and blob alike). Istio-agent `WASM_HTTP_REQUEST_TIMEOUT` parity. |
| `wasm_oci_pull_attempts` | `u32` | `3` | Total attempts (including the first) for each OCI pull request; transport errors and `5xx` retry, `4xx` never does. Istio-agent `WASM_HTTP_REQUEST_MAX_RETRIES` parity. |

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
> When mTLS is enabled, the URI scheme of `gateway.server_addr` is automatically switched to `https://`.
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
- `lru_cache` supplies process-wide plugin cache defaults.
- `all_endpoint_status` sets security ceilings for that plugin.
- `path_normalization` is process-local and restart-bound. It is not an
  `EdgionGatewayConfig` CRD field.

### server

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `threads` | `usize` | CPU core count | Pingora worker thread count |
| `work_stealing` | `bool` | `true` | Tokio task stealing |
| `grace_period_seconds` | `u64` | `30` | Graceful shutdown wait time in seconds |
| `graceful_shutdown_timeout_seconds` | `u64` | `10` | Shutdown timeout in seconds |
| `upstream_keepalive_pool_size` | `usize` | `128` | Upstream keepalive connection pool size |
| `downstream_keepalive_request_limit` | `u32` | `1000` | Maximum requests per downstream connection (0 = unlimited) |
| `error_log` | `String?` | — | Pingora internal error log path |

> Note: these are process-level default behaviors. If you are tuning GatewayClass-level server behavior, look at `EdgionGatewayConfig` instead.

### Business Log Schema

All business logs (access_log / ssl_log / tcp_log / tls_log / udp_log) share the same schema:

```yaml
LogConfig:
  enabled: bool                    # Whether enabled
  output:
    localFile:                     # The only output type currently
      path: String                 # Log file path (relative to work_dir)
      queue_size: usize?           # Write queue size (default: cpu_cores × 10000)
      rotation:                    # Rotation configuration (optional)
        strategy: RotationStrategy # daily | hourly | never | { Size: u64 }
        max_files: usize           # Number of files to retain (0 = unlimited)
        check_interval_secs: u64   # Rotation check interval (default 30s)
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

### access_log_sensitive

Controls the redaction behavior of fields marked as sensitive in access log, ssl.log, and plugin log. **`mode=off` by default**: all sensitive values are emitted in plaintext (preserving historical behavior).

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `Enum` | `"off"` | `"off"` / `"replace"` / `"wrap"` |
| `replace_with` | `String` | `"***"` | When `mode=replace`, replace the entire sensitive value |
| `wrap_prefix` | `String` | `"<E>"` | Prefix appended when `mode=wrap` |
| `wrap_suffix` | `String` | `"</E>"` | Suffix appended when `mode=wrap` |

**Behavior of the three modes** (using `client_addr=1.2.3.4` as an example):

| Mode | Output |
|------|--------|
| `off` | `"1.2.3.4"` |
| `replace` | `"***"` |
| `wrap` | `"<E>1.2.3.4</E>"` |

**Scope**:
- Access log fields: `client_addr` / `remote_addr` / `path` / `x_forwarded_for` / `sni` / client certificate `subject` / `sans` / `cn`.
- Plugin log: parameters annotated via `sensitive(...)`; user parameters of DSL `log_warn` / `log_error`.
- ssl.log: client certificate fields in mTLS allowlist validation failure errors.

**Implementation details**:
- A global `AtomicU8` controls the mode, written once by `init_sensitive` at startup; immutable at runtime.
- When `off` (the default), there is only a single atomic load overhead, effectively zero cost.
- Wrap mode uses a thread-local buffer with no heap allocation.
- For custom wrap prefixes/suffixes, the total length should not exceed 36 bytes; otherwise it reduces the per-entry writable length of the plugin log.

### require_fips

Gateway-local (not cluster-synced) startup guard for BoringSSL FIPS builds. Also settable via
env `EDGION_REQUIRE_FIPS` (truthy: `1`/`true`/`yes`/`on`); the env takes effect the same as the
config field.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `require_fips` | `bool` | `false` | Require the FIPS-validated crypto module to be present at startup |

- `require_fips: false` (default): starts normally regardless of whether the binary is a FIPS
  build.
- `require_fips: true` on a FIPS build (image compiled with the `fips` cargo feature): the guard
  passes; the FIPS-approved default cipher/curve set applies automatically to downstream TLS.
- `require_fips: true` on a non-FIPS build (validated module absent): the gateway logs a clear
  error and exits non-zero at startup (fail-close) — no silent downgrade.

This is a gateway-local field, not a cluster-synced one: a bad value can't be hot-pushed to
crashloop a fleet, and it surfaces on first run. The `fips` cargo feature itself is an image/build
concern, not something toggled at runtime. See
`docs/superpowers/specs/2026-07-10-fips-host-detection-design.md` for the full design, including the
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
| `fips_policy` | `"auto" \| "on" \| "off"` | `auto` | Select the FIPS-approved cipher/curve enforcement regardless of the validated module |

- `auto` (default): follow the host kernel. Enforces the approved set when the host kernel reports
  FIPS mode via `/proc/sys/crypto/fips_enabled`; otherwise does not. This is cipher-suite policy
  alignment only — it is **not** a validated-module compliance claim.
- `on`: always enforces the approved cipher/curve set, even on a normal (non-FIPS) boringssl build.
  Two uses: (1) exercise the FIPS policy in integration tests without a FIPS toolchain/module (the
  approved-suite behavior is pure SSL config, so it works on any boringssl build); (2) opt into the
  approved-suite restriction as defense-in-depth.
- `off`: never force-enables the policy. On a genuine FIPS build the approved profile still applies
  unconditionally — `off` can only disable the module-less (forced) half of the gate, never real
  FIPS-build enforcement. Effective gate is `fips_module_available() OR fips_policy_forced` where
  `fips_policy_forced` is derived from this setting (and, for `auto`, from the host state).
  Operator `EdgionTls.ciphers` still overrides the cipher list (approved curves still apply). This
  field does **not** provide the validated crypto module — for real FIPS compliance the binary must
  still be a FIPS build.

**Accepted values.** Only the three strings above — no legacy booleans, no legacy truthy/falsey
tokens. A YAML boolean (`fips_policy: true`) is a hard deserialize error. YAML 1.1 reads a bare
`on` / `off` scalar as a boolean, so those two values must be quoted: `fips_policy: "on"` /
`fips_policy: "off"`; `auto` needs no quoting.

**Resolution order, highest first: env → CLI → config file → `auto`.**

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

- Change the Controller gRPC address: change `gateway.server_addr`
- Change system log noise: change `logging.log_level`
- Toggle business logs: change `access_log` / `ssl_log` / `tcp_log` / `udp_log`
- Tune Pingora workers and keepalive: change `server`
- Tune the default RateLimit precision: change `rate_limit`
- Require the FIPS-validated crypto module at startup (FIPS builds only): change `require_fips` or set `EDGION_REQUIRE_FIPS`
- Tune the Wasm `source.oci` disk cache / pull behavior: change `gateway.wasm_oci_cache_dir` / `wasm_oci_cache_ttl_secs` / `wasm_oci_cache_purge_interval_secs` / `wasm_oci_cache_max_bytes` / `wasm_oci_pull_timeout_secs` / `wasm_oci_pull_attempts`
