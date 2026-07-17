# Log safety rules

> Three iron rules: do not leak secrets, do not leak configuration, no tracing on the data plane.

## Iron rule 1: no sensitive information in logs

### Strictly forbidden

| Category | Examples | Handling |
|------|------|---------|
| Plain-text Token / API Key / Secret | JWT token, HMAC key, API key | Record only the first 8 characters + `***`, or only the hash |
| Password / certificate private key | TLS private key, DB password | Do not record at all; only record the operation result (success/failure) |
| Full certificate text | PEM content | Record only fingerprint / subject / SAN |
| User PII | Name, email, phone | Do not record, or mask (e.g. `j***@example.com`) |
| Full request/response body | POST body, response payload | Record only Content-Length / Content-Type |
| ACME challenge token | HTTP-01 token | Do not record in tracing at all; only record the request path via access log |

### Handle with care

| Category | Risk | Handling |
|------|------|---------|
| `client_cert_info` | subject / SAN may contain PII | Already wired into the `access_log_sensitive` global switch via `serialize_with = "masked::serialize"` |
| `client_addr` / `remote_addr` / `path` / `x_forwarded_for` / `sni` | Real user IP / request path | Same as above; uniformly masked when `mode=replace` or `mode=wrap` is enabled |
| Plugin log dynamic parameters | May contain username / IP / token | Use `push_fmt` + `sensitive(...)` to mark explicitly; DSL `log_warn` / `log_error` automatically go through `push_sensitive` |

### Code examples

```rust
// Safe
tracing::info!(kind = "Secret", name = %name, "Secret loaded successfully");
log.push(&format!("OK k={}***", &api_key[..api_key.len().min(8)]));

// Dangerous
tracing::info!(secret_data = %secret.data, "Secret content");
tracing::info!(token = %full_token, "ACME challenge token");
log.push(&format!("OK token={}", full_jwt_token));
```

### PluginLog error entries: must use `push_err`

> **API spec + examples** are in the "Error-entry API constraints" subsection of [`observability/00-access-log.md`](observability/00-access-log.md) (authoritative source). This section explains **why** the constraint exists.

**Mandatory rule**: plugin error paths must always go through `plugin_log.push_err(kind: &'static str)`. **Forbidden**: `plugin_log.push(&format!("...", err))` / `push(&err.to_string())` / `push(any_dynamic_string)`.

**Why the `&'static str` signature**:

1. **Compile-time interception of `format!` / `to_string()` / local `&str`** — most leakage paths (the Display of `jsonwebtoken::Error` may carry token fragments, `reqwest::Error` may carry full URLs, claim values may be formatted into strings) fail to compile at the call site, so we don't have to rely on review eyeballs.

2. **Only literals and `const` are allowed** (technically `Box::leak(format!(...).into_boxed_str())` also passes, but that pattern is grep-able at a glance during review and is not a real threat).

3. **Forces developers to `match` first, then write the label**: `jwt_err.kind()` → a fixed set of static strings. The classification step is moved into the code, ensuring kind is a bounded set at the source and making downstream access-log aggregation by kind easier.

4. **No intermediate String allocation**: `push_err` writes via `extend_from_slice(&'static [u8])` calls — no `format!`-induced heap allocation for the entry content, even when tier-2 is in use — so it stays usable on the data-plane hot path.

**No double-write**: for the same error, write either `push_err` (classification) or `ctx.err_log` (masked detail) — pick one. Writing both means duplicating sensitive content twice.

---

## Iron rule 2: no configuration leakage in logs

### Things that should not be printed at info level

| Content | Log level | Reason |
|------|---------|------|
| Full resource spec | debug/trace only | May contain Secret references, internal addresses |
| Backend IP:Port list | debug only | Internal network topology |
| Detailed routing rules | debug only | Business routing logic |
| Plugin config JSON | debug only | May contain key configuration |
| Health-check addresses | debug only | Exposes internal endpoints |

### Allowed at info level

| Content | Example |
|------|------|
| Resource key_name | `namespace/name` |
| Resource count | `count=42` |
| Event type | `add` / `update` / `delete` |
| rv / sv | Version numbers |
| Error summary | Error type + short description |

### Code examples

```rust
// info level: record only key and count
tracing::info!(kind = "HTTPRoute", key = %key, rv, "Resource processed");
tracing::info!(kind = "EdgionTls", count = data.len(), "Full set completed");

// info level: leaks the full configuration
tracing::info!(spec = ?route.spec, "Route spec details");
tracing::info!(backend = ?backend_ref, "Selected backend");  // Includes IP/Port
```

---

## Iron rule 3: no tracing logs on the data plane

### Principle

The data plane (request/connection processing path) produces logs only through the following mechanisms:

| Mechanism | Purpose | Example |
|------|------|------|
| **access log** | Request-level result records | `AccessLogEntry` / `log_tls()` |
| **ctx.err_log / ctx.log** | Connection-level error/event | TLS proxy's `ctx.err_log = Some(...)` |
| **PluginLog** | Plugin execution summary | `log.push("OK u=alice")` |
| **Metrics** | Aggregated counters | `global_metrics().ctx_created()` / `record_backend_request(...)` |

**Any `tracing::` macro is forbidden on the request/connection processing hot path**, including `debug!`.

Long-running Controller/Gateway services must not use `println!` / `eprintln!` after
tracing initialization; use structured tracing on cold/control paths. CLI user-facing
stdout/stderr and pre-tracing startup/fatal fallbacks are intentional exceptions.

### Reasons

1. **Performance**: tracing has lock contention under high concurrency (`debug!` can be filtered out by `EnvFilter`, but the format overhead remains)
2. **Log explosion**: one tracing log per request/connection × QPS = an enormous log volume
3. **Information duplication**: access log / ssl log / tcp log already cover the request/connection-level information
4. **Correlation**: tracing logs cannot be correlated to a specific request, but access logs can

### Data-plane entry list (boundary criterion)

> **Decision rule**: every call chain originating from any entry below is a data-plane hot path; tracing is forbidden (use only ctx/access log/metrics).
> Code outside these entries (construction, assembly, sync, startup, background workers) is judged in context — most can use tracing.

**HTTP request path** — `impl ProxyHttp for EdgionHttpProxy` (`gateway/routes/http/proxy_http/mod.rs`)

| Callback | Dispatch file |
|------|----------|
| `new_ctx` / `init_downstream_modules` | `pg_new_ctx.rs` / `pg_init_downstream_modules.rs` |
| `early_request_filter` / `request_filter` / `request_body_filter` | `pg_early_request_filter.rs` / `pg_request_filter.rs` / `pg_request_body_filter.rs` |
| `upstream_peer` / `connected_to_upstream` | `pg_upstream_peer.rs` / `pg_connected_to_upstream.rs` |
| `upstream_response_filter` / `upstream_response_body_filter` / `response_filter` | `pg_upstream_response_filter.rs` / `pg_upstream_response_body_filter.rs` / `pg_response_filter.rs` |
| `fail_to_connect` / `error_while_proxy` / `fail_to_proxy` / `logging` | `pg_fail_to_connect.rs` / `pg_error_while_proxy.rs` / `pg_fail_to_proxy.rs` / `pg_logging.rs` |

The HTTP→HTTPS redirect service (`ProxyHttp` impl in `routes/http/redirect_http.rs`) is equivalent.

**TCP/TLS connection path** — `process_new` of `impl ServerApp`

| File | Scenario |
|------|------|
| `gateway/routes/tcp/edgion_tcp.rs` | TCP |
| `gateway/routes/tls/passthrough_proxy.rs` | TLS passthrough |
| `gateway/routes/tls/proxy.rs` | TLS termination + TCP forward |

**UDP connection path** (**not** a Pingora trait — own loop, per-packet entry) — `gateway/routes/udp/edgion_udp.rs`
- The `recv_from` loop in `EdgionUdpProxy::serve()`
- `handle_client_packet` / `handle_upstream_packets` (per-session handlers spawned via tokio::spawn)

**TLS handshake callbacks** — `impl TlsAccept for TlsCallback` (`gateway/tls/runtime/gateway/tls_pingora.rs`)
Includes `certificate_callback` / `handshake_complete_callback`.

**BoringSSL low-level callbacks** (within the synchronous TLS-handshake call chain):
- `tls/boringssl/mtls_verify_callback.rs` — `extern "C" fn verify_callback` / `verify_callback_impl` (mTLS allowlist check, fires per handshake)
- `tls/runtime/backend/cert_extractor.rs` (extracts client certificate within `handshake_complete_callback`)

**Plugin dispatch** (extension of the callbacks above; all counted as data plane):
- `run_*_plugins` in `plugins/runtime/plugin_runtime.rs`
- `RequestFilter` / `UpstreamResponse*` implementations under `plugins/runtime/traits/**`
- `plugins/runtime/conditional_filter.rs`, `plugins/runtime/session_adapter.rs`, `plugins/runtime/gateway_api_filters/**`
- `filter` / `apply` / `on_request` methods of `plugins/http/**/plugin.rs`
- `plugins/stream/stream_plugin_runtime.rs`, `plugins/stream/connection_filter_bridge.rs` (`impl ConnectionFilter`), `plugins/stream/*/plugin.rs` (`impl StreamPlugin`)
- `plugins/stream/tls_route/tls_route_plugin_runtime.rs`, `plugins/stream/tls_route/*.rs` (`impl TlsRoutePlugin`, route plugins after the TLS handshake)

**Shared modules entered via the callback chain** (data plane when called from there; not when called only at config/startup time):
- `routes/http/match_engine/{radix_route_match,regex_routes_engine}.rs`, `routes/http/match_unit.rs` (HTTP route matching, called from `request_filter`)
- `routes/grpc/{match_engine,match_unit,integration}.rs` (gRPC matching, reuses HTTP proxy's `request_filter`)
- `routes/http/session_persistence.rs` (sticky session in `upstream_peer`)
- `lb/selection/{round_robin,least_conn,ewma,consistent_hash}.rs`, `lb/runtime_state/mod.rs` (per-call backend selection in `upstream_peer`)
- `backends/mod.rs` peer selection, `backends/validation.rs` SSRF check, `backends/circuit_breaker/store.rs` circuit-breaking decision, `backends/policy/` per-request policy
- `cache/**` per-request lookup
- SNI→cert matching in `tls/store/cert_matcher.rs` (called by `certificate_callback`)
- `runtime/server/error_response.rs` (response generated when `fail_to_proxy` / `request_filter` reject)

#### Explicitly **not** data plane (even under `gateway/`)

| Location | Reason |
|------|------|
| `plugins/**/conf_handler_impl.rs`, `PluginRuntime::build_*` | Plugin construction during config/sync; one-shot |
| `plugins/**::new()` / `::from_config()` | Constructors, not request path |
| `conf_sync/**`, LinkSys configuration/store management | Control/config paths; however LinkSys provider calls reached from DSL/plugins are request hot paths and remain tracing-free |
| `api/**` | Admin API (control plane) |
| Startup/shutdown under `runtime/server/**` | One-shot events |
| Registration in `observe/metrics/registry.rs` | Startup phase |
| Background tasks under `services/acme/**`, `backends/health/` health-probe loop, `backends/discovery/` periodic tasks | Background workers, paced by job frequency rather than QPS |
| Cert loading under `tls/store/**`, parsing under `tls/validation/` | Configuration loading |

### Exceptions

| Scenario | Allowed | Reason |
|------|------|------|
| Config loading/changes (`ConfHandler`/`conf_handler_impl`) | `tracing::info!` | Not on the request path; counts as a system event |
| Data-plane startup/shutdown (process-level `once_cell` initialization) | `tracing::info!` | One-shot event |
| Fatal errors (before panic) | `tracing::error!` | Extreme cases need to be visible immediately |
| Access log send failure in `pg_logging.rs` | `tracing::warn!` | log store issues need system-level visibility |
| `catch_unwind` panic-capture branch in `mtls_verify_callback.rs` | `tracing::error!` | A Rust panic inside a C callback is UB and must be visible immediately |
| `proxy_http/parse_timeout.rs` (`ParsedTimeouts::from_config`) | `tracing::warn!` | Only invoked while parsing the `EdgionGatewayConfig.httpTimeout` string at listener build time; once per reload. The file is under `proxy_http/` but is not on the request path |
| `plugins/http/basic_auth/plugin.rs` (`load_users` warn + constructor Loaded info) | `tracing::warn!` / `tracing::info!` | htpasswd parsing / duplicate-username deduplication / Loaded summary called by `BasicAuth::new`; only triggered during config loading |
| `plugins/http/header_cert_auth/plugin.rs` (drop expired / unparseable CA in `load_ca_certs`) | `tracing::warn!` | The CA trust pool is assembled only at plugin construction; the drop decision does not participate in per-request validation |

### Code examples

```rust
// TLS proxy: use ctx-based logging
ctx.err_log = Some(format!("No matching TLSRoute for SNI={sni}"));

// HTTP proxy: errors flow into the access log
ctx.add_error(EdgionStatus::NoRoute);

// Plugin: via PluginLog
log.push("Deny rate-limited");

// Data-plane hot path: do not use tracing
tracing::warn!("No route found for {}", host);  // Logged per request — log explosion
tracing::info!("Selected backend: {:?}", backend_ref);  // Leakage + performance
```

---

## Checklist

Use during code review:

- [ ] Any `println!` / `eprintln!` in a long-running service after tracing init? Replace it; do not flag CLI output or pre-init fallback blindly
- [ ] Are newly added `tracing::info!/warn!/error!` calls on the data-plane hot path? If so, switch to ctx-based
- [ ] Do logs contain sensitive data such as Secret / Token / keys?
- [ ] Do logs contain a full resource spec / backend address? If so, downgrade to debug
- [ ] Do new control-plane logs include `kind` + `name/namespace` + `rv`? (Can be omitted inside a span — provided automatically by the span)
- [ ] If the resource has been synced to the data plane, is `sv` present in the access log?
- [ ] Does a new `ctx_map` key store claims/decrypted payloads/tokens/secrets? Add it to `ACCESS_LOG_BLOCKED_CTX_KEYS` (`edgion-resources/src/resources/edgion_plugins/plugin_configs/access_log_extern.rs`) so no `AccessLogExtern from: ctx` declaration can ever project it. This is review-blocking.
- [ ] Do plugin stage logs record a full username/token or unmasked dynamic error? This is review-blocking

---

## Periodic maintenance (run quarterly or after each "data-plane entry list" change)

> **Automated gate**: `cicd/checks/check_gateway_no_tracing.py` resolves the enclosing
> function so cold constructors/config builders can trace while hot callbacks cannot.
> Run it (and `--list`) rather than relying on a prose baseline. Any unregistered output
> is live debt to fix, not an implied exemption. At the time of this update the list still
> reports Wasm plugin hot-path calls; do not copy that pattern into new code. Approved
> exceptional sites require an inline `// tracing-allow: <reason>` and review. Keep the
> gate's function/directory coverage synchronized with this boundary, including provider
> operations invoked by plugins.

**1. Scan data-plane entry files for newly introduced tracing** (note: only check **the entry files themselves** — `conf_handler_impl.rs` / `routes_mgr.rs` / `conf_handler.rs` in those directories belong to the config phase, do not scan the whole directory):

```bash
rg -n 'tracing::' \
  edgion-gateway/src/routes/http/proxy_http/ \
  edgion-gateway/src/routes/http/match_engine/ \
  edgion-gateway/src/routes/http/match_unit.rs \
  edgion-gateway/src/routes/http/session_persistence.rs \
  edgion-gateway/src/routes/http/redirect_http.rs \
  edgion-gateway/src/routes/tcp/edgion_tcp.rs \
  edgion-gateway/src/routes/tls/passthrough_proxy.rs \
  edgion-gateway/src/routes/tls/proxy.rs \
  edgion-gateway/src/routes/udp/edgion_udp.rs \
  edgion-gateway/src/routes/grpc/match_engine.rs \
  edgion-gateway/src/routes/grpc/match_unit.rs \
  edgion-gateway/src/routes/grpc/integration.rs \
  edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs \
  edgion-gateway/src/tls/runtime/backend/backend_api.rs \
  edgion-gateway/src/tls/runtime/backend/cert_extractor.rs \
  edgion-gateway/src/tls/boringssl/mtls_verify_callback.rs \
  edgion-gateway/src/tls/store/cert_matcher.rs \
  edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs \
  edgion-gateway/src/plugins/stream/connection_filter_bridge.rs \
  edgion-gateway/src/plugins/stream/tls_route/ \
  edgion-gateway/src/plugins/stream/ip_restriction/ \
  edgion-gateway/src/plugins/stream/global_connection_ip_restriction/ \
  edgion-gateway/src/plugins/runtime/plugin_runtime.rs \
  edgion-gateway/src/plugins/runtime/conditional_filter.rs \
  edgion-gateway/src/plugins/runtime/session_adapter.rs \
  edgion-gateway/src/plugins/runtime/gateway_api_filters/ \
  edgion-gateway/src/plugins/http/*/plugin.rs \
  edgion-gateway/src/plugins/http/*/request_filter_impl.rs \
  edgion-gateway/src/lb/selection/ \
  edgion-gateway/src/lb/runtime_state/ \
  edgion-gateway/src/backends/mod.rs \
  edgion-gateway/src/backends/validation.rs \
  edgion-gateway/src/backends/circuit_breaker/store.rs \
  edgion-gateway/src/backends/policy/ \
  edgion-gateway/src/cache/ \
  edgion-gateway/src/runtime/server/error_response.rs
```

**Expected**: only the exempt items listed in the exception table remain (the access-log-send failure warn in `pg_logging.rs`, the panic-capture error in `mtls_verify_callback.rs`, etc.). Any new hit must either:
- be switched to ctx-based log / access log / PluginLog / metrics, **or**
- be added to the exception table after review with a stated reason.

`parse_timeout.rs` is on the config-loading path (`ParsedTimeouts::from_config` is called
only at listener build time) and may trace as a cold function.

**2. Verify that no new unregistered entries have been added to Pingora / plugin trait impls**:

```bash
# Data-plane trait impls — every result's file path must match an entry in the "Data-plane entry list" table or the "Plugin dispatch" subsection
rg -n 'impl (ProxyHttp|ServerApp|TlsAccept|ConnectionFilter|StreamPlugin|TlsRoutePlugin|RequestFilter|UpstreamResponseFilter|UpstreamResponseBodyFilter|UpstreamResponse) for ' \
  edgion-gateway/src/
```

**Expected**: every hit is covered under "HTTP request path / TCP/TLS connection path / UDP connection path / TLS handshake callbacks / Plugin dispatch" in this rule. New trait impls must update the tables above; otherwise the next review-ticket triage will miss them.

---

## Resolved cleanup log (historical)

All previously-identified hot-path tracing / sensitive-data items below have been cleared.
This table is kept as a historical record; there are no remaining open items in it.

| File | Issue | Fix | Status |
|------|------|---------|------|
| ~~Full HTTP proxy path~~ | ~~tracing exists in `pg_upstream_peer` and several other files~~ | ~~Remove all~~ | Fixed |
| ~~`tls_pingora.rs`~~ | ~~Heavy tracing on the TLS handshake path~~ | ~~Remove all~~ | Fixed |
| ~~`edgion_tcp.rs`~~ | ~~Tracing on the TCP connection path~~ | ~~Remove all~~ | Fixed |
| ~~`stream_plugin_runtime.rs`~~ | ~~Stream plugin deny info log~~ | ~~Remove~~ | Fixed |
| ~~`stream_ip_restriction.rs`~~ | ~~Connection-level debug/info~~ | ~~Remove all~~ | Fixed |
| ~~`connection_filter_bridge.rs`~~ | ~~Connection-filter debug/info~~ | ~~Remove all~~ | Fixed |
| ~~`grpc/match_unit.rs`~~ | ~~Request-level warn~~ | ~~Remove~~ | Fixed |
| ~~`preflight_handler.rs`~~ | ~~preflight debug~~ | ~~Remove~~ | Fixed |
| ~~Access log `ctx_map`~~ | ~~Sensitive claim keys (`jwt_claims` / `oidc_claims` / `jwe_payload`) flattened verbatim into the access log~~ | Ctx vars reach the log only via an explicit `AccessLogExtern from: ctx` declaration; the three sensitive keys stay hard-blocked (`ACCESS_LOG_BLOCKED_CTX_KEYS`) even if declared. (Superseded the earlier per-route ctx-key allowlist annotation / `AllowlistedCtx`, both deleted.) | Fixed |
| ~~LDAP/JWT auth plugin username~~ | ~~username flowing into stage_logs~~ | Wired into the `access_log_sensitive` global masking switch with explicit `sensitive(...)` markers | Fixed |
