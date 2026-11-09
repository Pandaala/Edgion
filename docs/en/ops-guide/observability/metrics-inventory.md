# Metrics Inventory

> Authoritative catalog of every `edgion_*` metric. CI guard
> `cicd/checks/check_metrics_inventory.py` fails on any drift between this file and
> the names emitted in code (both directions). One row per metric.
>
> **Category** is `self` (Edgion's own health/infra) or `business` (outcomes of the
> tenant traffic through the gateway). It is documentation only — never a runtime
> Prometheus label. See the design spec
> docs/superpowers/specs/2026-07-20-metrics-observability-hardening-design.md §2.
>
> **Process** shows which binary emits the metric: `gateway`, `controller`, or
> `both` (emitted by the same shared code path from either side, e.g. federation
> connection lifecycle). Federation metrics that are Center-only (e.g.
> `edgion_fed_watch_list_total`, `edgion_fed_aggregator_*`) are not listed here —
> the Center process now lives in the separate EdgionCenter repository and is out
> of scope for this guard.

**Lifetime** is enforced by the canonical Common catalog and facade. `fixed`
families have closed label domains and never idle-expire; CRD, service and module
identities must be `dynamic`. Dynamic accumulation expires after inactivity;
already-admitted keys can renew through the same descriptor. Descriptor preparation
does not spend quota or expose zero series. The single lifetime soft cap defaults
to 10,000 keys, includes both classes and the guard's two self metrics, and never
refunds expired identities. Valid fixed tuples are accepted at saturation; new
dynamic tuples are dropped in enforce mode. Observe mode and cap zero do not
promise a hard memory cap. `snapshot` names are the two separately bounded
Controller hand-rendered families and remain outside recorder admission.

Classic request-duration buckets multiply wire samples. Empty bucket overrides
and Wasm duration families preserve the exporter's rolling summary wire format
(three 20-second windows; cumulative count/sum). Aggregation has a fixed storage
bound rather than retaining raw samples. Dynamic CRD churn still consumes the
lifetime ledger, so deleted timestamp-named routes cannot continuously recycle
quota. See [the lifetime design](../../dev-guide/metrics-lifetime-design.md).

For generic Watch decode failures, the Gateway emits one error per failed response
with the first failure's `kind`, `ns`, `name`, `resourceVersion`, `syncVersion`, and
safe error summary. Unavailable or invalid identity fields are omitted; top-level
JSON failures use the response sync version. Resource bodies and raw decoder error
values are excluded. The first decode failure increments the counter and ends the
current Watch; its remaining events are not processed or counted. The Gateway
retries List then Watch with the existing backoff. Successfully applied events and
readiness are retained while waiting for a valid replacement snapshot. The signal
does not mean recovery succeeded: persistent schema incompatibility can also fail
List, so compare Controller/Gateway builds and investigate the named resource.
EdgionBackend retains its separate withdrawal policy.

## Self

| metric | type | process | labels | category | lifetime | description | alert |
|--------|------|---------|--------|----------|----------|-------------|-------|
| edgion_ctx_created_total | Counter | gateway | none | self | fixed | Total request contexts created (requests received) | - |
| edgion_ctx_active | Gauge | gateway | none | self | fixed | Currently active in-flight request contexts | - |
| edgion_downstream_connections_active | Gauge | gateway | none | self | fixed | Currently open downstream sockets admitted by a Pingora listener (HTTP/HTTPS/TCP/TLS; UDP excluded), from successful listener admission until the l4 stream drops. Sockets still in listener admission count toward `server.max_downstream_connections` but not here, so this gauge can read slightly below the cap while global rejections increase | - |
| edgion_access_log_dropped_total | Counter | gateway | none | self | fixed | Dropped HTTP access-log entries (backpressure/failure on the log-shipping path) | - |
| edgion_ssl_log_dropped_total | Counter | gateway | none | self | fixed | Dropped SSL/TLS-termination log entries | - |
| edgion_tcp_log_dropped_total | Counter | gateway | none | self | fixed | Dropped TCP stream access-log entries | - |
| edgion_tls_log_dropped_total | Counter | gateway | none | self | fixed | Dropped TLS-route access-log entries | - |
| edgion_udp_log_dropped_total | Counter | gateway | none | self | fixed | Dropped UDP stream access-log entries | - |
| edgion_access_log_backfill_ingested_total | Counter | gateway | none | self | fixed | Access-log documents successfully re-ingested into Elasticsearch from the local fallback file during a backfill run | - |
| edgion_access_log_backfill_failed_total | Counter | gateway | none | self | fixed | Access-log lines that failed to backfill into Elasticsearch and are kept on disk for a later retry | - |
| edgion_config_reload_signals_total | Counter | gateway | none | self | fixed | Reload signals received by the Gateway from the Controller over conf_sync | - |
| edgion_config_relist_total | Counter | gateway | none | self | fixed | Config relist operations performed by the Gateway's conf_sync client | - |
| edgion_config_event_parse_errors_total | Counter | gateway | kind | self | fixed | Watch decode failures per kind; generic Watch counts its first failure and relists without decoding the remaining events | - |
| edgion_gateway_waf_compile_total | Counter | gateway | result (ok/error) | self | fixed | Native WAF generation compilation attempts; rule or resource identity stays out of labels | alert on sustained `rate(...{result="error"}[5m]) > 0` |
| edgion_gateway_waf_compile_microseconds_total | Counter | gateway | result (ok/error) | self | fixed | Cumulative native WAF compilation time in microseconds; divide its rate by `edgion_gateway_waf_compile_total` for mean build latency | - |
| edgion_gateway_waf_generation_bytes | Gauge | gateway | none | self | fixed | Retained bytes charged to live compiled native WAF generations, including generations pinned by in-flight requests | alert near the configured process memory envelope |
| edgion_gateway_waf_runtime_bytes_active | Gauge | gateway | none | self | fixed | Logical request-owned WAF transaction/scratch charge plus known current provisional JSON allocation capacities; excludes BodyStorage, inline storage, allocator overhead and RSS | alert on sustained growth disproportionate to active request count |
| edgion_gateway_waf_body_bytes_inspected_total | Counter | gateway | direction (request/response) | self | fixed | Request source bytes admitted once: prefix bytes, Full non-JSON submitted bytes, or exact JSON lexer deltas (stops on inspection failure); excludes replay/per-rule multiplication; response reserved until Phase 4 | - |
| edgion_gateway_waf_execution_total | Counter | gateway | phase (request_headers/request_body/response_headers/response_body/logging) | self | fixed | Native WAF transaction executions by fixed phase | - |
| edgion_gateway_waf_execution_microseconds_total | Counter | gateway | phase (request_headers/request_body/response_headers/response_body/logging) | self | fixed | Cumulative native WAF phase execution time, excluding request capture/JSON publication; divide its rate by `edgion_gateway_waf_execution_total` for mean latency | - |
| edgion_request_body_storage_io_failures_total | Counter | gateway | operation (spill_create/spill_write/replay_read/materialize_read/snapshot_read), reason (admission_timeout/admission_closed/execution_timeout/join_error/io_error) | self | fixed | Request-body storage operation failures. Admission timeout means the fixed storage lane was saturated before work reached the blocking pool; execution timeout means admitted work exceeded its deadline | alert on sustained increase, especially `reason="admission_timeout"` |
| edgion_request_body_storage_io_inflight | Gauge | gateway | none | self | fixed | Storage file operations holding one of the fixed process-wide permits, including work queued in the blocking pool | alert when pinned at 32 together with increasing admission timeouts |
| edgion_link_sys_es_bulk_channel_full_total | Counter | gateway | none | self | fixed | Elasticsearch producer→bulk-loop channel was full at submit time; raise `bulk.channelSize` | - |
| edgion_link_sys_health | Gauge | gateway | link | self | dynamic | Published LinkSys client health sampled at scrape time: 1 = healthy, 0 = unhealthy; a deleted client's last series may remain until idle eviction or process exit | - |
| edgion_link_sys_health_flips_total | Counter | gateway | link, direction (recovered/degraded) | self | dynamic | Health transitions from the currently published Redis, Etcd, Elasticsearch, Kafka, and OTLP LinkSys generations; candidate initialization, publication/replacement baselines, deletion, duplicate observations, and retired generations do not increment it | - |
| edgion_link_sys_redis_connections | Gauge | gateway | link, state (live/configured) | self | dynamic | Published Redis LinkSys connection coverage sampled at scrape time. Standalone/Sentinel: live pool members vs `pool.size`. Cluster: primaries answering the latest probe vs known primaries. `live < configured` means partial degradation; a deleted client's last series may remain until idle eviction or process exit | - |
| edgion_link_sys_redis_topology_events_total | Counter | gateway | link, event (recovery_failed/recovered/primary_changed) | self | dynamic | Redis background-recovery transitions from the currently published generation: first failure after success, success after failure, and a Sentinel primary replaced by a different one. Repeated failures count once; candidates and retired generations do not count. No endpoint labels | - |
| edgion_ldap_auth_errors_total | Counter | gateway | kind (unreachable/timeout/bind_fail/tls_san_mismatch/service_bind_fail/search_fail) | self | fixed | LDAP auth plugin error counter by failure kind | - |
| edgion_gateway_outbound_tls_san_mismatch_total | Counter | gateway | plugin (ldap_auth/openid_connect) | self | fixed | Outbound TLS SAN-pinning rejection — signals a misconfigured `validation.subjectAltNames` or a rotated peer certificate | - |
| edgion_webhook_resolve_errors_total | Counter | gateway | reason (not_found/unhealthy/rate_limited/status_error/call_failed/authority_rejected) | self | fixed | Webhook key-resolver per-resolve failure counter by reason | - |
| edgion_outbound_egress_blocked_total | Counter | gateway | reason (loopback/link_local/private/unspecified) | self | fixed | Outbound address rejected by the SSRF egress policy, by reason. Counted on every use of a resolved answer (answers are cached raw and classified per use) | - |
| edgion_gateway_upstream_authority_rejected_total | Counter | gateway | reason (render_empty/invalid_label) | self | fixed | Request-time upstream authority the Gateway refused to use (empty template render, or the rendered value is not a valid DNS label) | - |
| edgion_outbound_success_predicate_fail_total | Counter | gateway | none | self | fixed | Outbound response accepted at the status layer but classified as failure by a `success.body` predicate ("200 OK but error in the body") | - |
| edgion_outbound_template_fallback_total | Counter | gateway | field (timeout_ms) | self | fixed | A templated outbound field failed to render and silently fell back to its static default | - |
| edgion_config_conflict_total | Counter | gateway | field (constant set: outbound_ca, outbound_client_identity) | self | fixed | A config-aggregation conflict selected a conservative result: conflicting outbound CA defaults fall back to System, while conflicting outbound client identities make inherited TLS client construction fail closed. Zero-event sentinel | alert on `increase(...) > 0` |
| edgion_outbound_injected_header_skipped_total | Counter | gateway | none | self | fixed | A caller-injected outbound header (e.g. X-Forwarded-* from ForwardAuth) was dropped for an invalid name/value — the operator's configured header silently no-ops. Zero-event sentinel | alert on `increase(...) > 0` |
| edgion_gateway_fips_module_enabled | Gauge | gateway | none | self | fixed | 1 = BoringSSL reports FIPS mode in this binary; this does not establish CMVP certificate scope or build provenance | `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0` |
| edgion_gateway_fips_host_enabled | Gauge | gateway | none | self | fixed | 1 = the host kernel reports FIPS mode (`/proc/sys/crypto/fips_enabled`) | `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0` |
| edgion_gateway_fips_host_detect_error | Gauge | gateway | none | self | fixed | 1 = the host FIPS flag exists but could not be read (masked `/proc`); both the guardrail and `fips_policy=auto` are inert in this state | - |
| edgion_gateway_fips_policy_active | Gauge | gateway | module_enabled (true/false) | self | fixed | 1 = the approved FIPS cipher/curve profile is in effect on downstream listeners; the label records the BoringSSL FIPS-mode signal | - |
| edgion_wasm_module_load_total | Counter | gateway | module, result (ok/error) | self | dynamic | Wasm module load attempt outcome; every Wasm `module` label uses the source descriptor, with URL sources represented by a credential-free, query-stable identity capped at 96 bytes | - |
| edgion_wasm_module_compile_duration_ms | Histogram | gateway | module | self | dynamic | Wall-clock duration of one Wasm `Module::new` compilation | - |
| edgion_wasm_module_released_total | Counter | gateway | module | self | dynamic | Compiled Wasm runtime generation released after its last holder exits | - |
| edgion_wasm_instance_instantiate_total | Counter | gateway | module | self | dynamic | Fresh Wasm VM instantiation (instance pool miss) | - |
| edgion_wasm_instance_discard_total | Counter | gateway | module, reason (poisoned/over_capacity) | self | dynamic | Wasm VM discarded instead of returned to the pool | - |
| edgion_wasm_pool_size | Gauge | gateway | module | self | dynamic | Current pooled Wasm instance count for a module | - |
| edgion_wasm_hook_duration_ms | Histogram | gateway | module, phase (request_headers/request_body/response_headers/response_body/http_call_response/tick) | self | dynamic | Duration of one Wasm guest hook invocation | - |
| edgion_wasm_trap_total | Counter | gateway | module, phase | self | dynamic | Wasm guest trap (or protocol violation that poisons the lease) | - |
| edgion_wasm_fail_mode_total | Counter | gateway | module, reason, mode (open/close) | self | dynamic | Fail-mode decision taken by the Wasm plugin host | - |
| edgion_wasm_callout_total | Counter | gateway | module, result (ok/error/denied/over_limit) | self | dynamic | Guest-dispatched HTTP callout outcome | - |
| edgion_wasm_modules_loaded | Gauge | gateway | none | self | fixed | Live compiled Wasm runtime generations, including initialization and outstanding old leases | - |
| edgion_wasm_shared_data_bytes | Gauge | gateway | module | self | dynamic | Sum of accounted bytes across all live stores sharing a module label, including overlapping generations; zero after the last store drops (4MiB cap per store, not per series) | - |
| edgion_wasm_body_pause_unsupported_total | Counter | gateway | module | self | dynamic | A Wasm guest returned `Action::Pause` from the response-body hook, which this host cannot honor (per-chunk window, not a cumulative buffer) — the guest silently observes only the current chunk | alert on this counter (no fixed threshold) |
| edgion_gateway_plugin_policy_blocked_total | Counter | gateway | plugin | self | dynamic | A request was rejected because its plugin chain contains an entry blocked by the captured GatewayClass policy | - |
| edgion_gateway_plugin_policy_bypassed_total | Counter | gateway | plugin | self | dynamic | A request passed through a chain with an entry bypassed by the captured GatewayClass policy | - |
| edgion_geoip_db_loaded | Gauge | gateway | none | self | fixed | 1 when a GeoIP database is loaded, 0 otherwise — the alerting signal for the dangerous silent failure where geo rules are configured but the security control is not running | alert on `edgion_geoip_db_loaded == 0` while geo rules are configured |
| edgion_geoip_db_build_epoch_seconds | Gauge | gateway | none | self | fixed | Build timestamp of the loaded GeoIP database | alert on staleness |
| edgion_geoip_update_failed_total | Counter | gateway | reason (fetch/encoding/size_cap/stall/validation/rename/disk/deadline) | self | fixed | GeoIP database update failures by class (background updater task only) | - |
| edgion_geoip_lookup_panic_total | Counter | gateway | none | self | fixed | A per-request MMDB lookup inside `GeoIpProvider::lookup` panicked and was reported as `Failed`. Distinct from `edgion_geoip_update_failed_total`, which is about the background update cycle. Decode-without-panic failures share that outcome but do not increment this counter | - |
| edgion_gateway_retry_budget_denied_total | Counter | gateway | none | business | fixed | Retries refused by a `retryConstraint` budget | - |
| edgion_conf_sync_cert_expiry_timestamp_seconds | Gauge | both | side (server/client) | self | fixed | Installed conf_sync certificate's `notAfter` (Unix seconds): Gateway samples its loaded client identity on each scrape; Controller samples its live server identity hourly. Rejected or stalled reloads retain the installed certificate's expiry | `edgion_conf_sync_cert_expiry_timestamp_seconds - time() < 30 * 86400` |
| edgion_system_error_state | Gauge | both | component, reason | self | fixed | `1` while a known degraded operating condition is active on this process (see `edgion-common/src/observe/system_metrics.rs` for the full bounded `component`/`reason` sets); some reasons latch until restart, others clear on the next success | latching: `== 1`; transient: sustained `== 1` with a `for:` guard |
| edgion_fed_connections_active | Gauge | controller | role (center/controller) | self | fixed | Currently active federation connections from the emitting process's point of view (Center side lives in the separate EdgionCenter repo) | - |
| edgion_fed_connection_events_total | Counter | controller | role, event (connected/disconnected/reload) | self | fixed | Federation session lifecycle events (this repo emits from the Controller only) | - |
| edgion_fed_connection_duration_seconds_last | Gauge | controller | role | self | fixed | Last observed federation session duration in seconds, recorded at disconnect | - |
| edgion_fed_watch_events_total | Counter | controller | kind, direction (sent/recv) | self | fixed | Watch events flowing across the federation link | - |
| edgion_fed_ready_gate_wait_seconds_last | Gauge | controller | none | self | fixed | Last observed ready-gate wait in seconds (Controller side) | - |
| edgion_fed_rbac_denied_total | Counter | controller | verb, kind, source (center/cli_token/unknown) | self | fixed | RBAC denials on the Controller admin-API federation path | - |
| edgion_fed_kill_switch_state | Gauge | controller | none | self | fixed | Federation kill-switch state — the operator's `center.enabled` intent only; 1 = enabled, 0 = disabled. Emitted on every supervisor apply | - |
| edgion_fed_client_running | Gauge | controller | none | self | fixed | Whether the federation client is actually running in this process; 1 = running, 0 = not running. `kill_switch_state == 1 and client_running == 0` is the normal steady state of a non-leader replica (only the leader may hold the federation session) | `edgion_fed_kill_switch_state == 1 and max(edgion_fed_client_running) == 0` (enabled but no replica is federating) |
| edgion_controller_resource_abnormal | Gauge | controller | kind, namespace, name, err, warn | self | snapshot | One series per resource with at least one failing status condition; `err`/`warn` are JSON arrays of the triggering condition types, capped at 1000 entries per snapshot | `edgion_controller_resource_abnormal{err!=""}` |
| edgion_controller_unknown_config_fields | Gauge | controller | kind | self | fixed | Number of `EdgionPlugins`, `EdgionStreamPlugins`, `EdgionConfigData`, or `LinkSys` resources whose submitted document contained a key the typed schema does not recognize, with one series per kind (advisory only, never blocks). Detailed bounded findings remain available through status, logs, and `GET /api/v1/diagnostics/unknown-fields`. Driven by a live scan of `spec.unknown_fields`, so — unlike `resource_abnormal` — it is populated in FileSystem/etcd mode too, not Kubernetes-mode only | `edgion_controller_unknown_config_fields > 0` |
| edgion_controller_init_sync_duration_seconds_last | Gauge | controller | kind | self | fixed | Duration of the last initial sync, per resource kind | - |
| edgion_controller_active | Gauge | controller | none | self | fixed | Number of active resource controllers | - |
| edgion_controller_resources_watched | Gauge | controller | kind | self | fixed | Resources currently watched, per kind | - |
| edgion_controller_leader | Gauge | controller | none | self | fixed | Leader election status: 1 = leader, 0 = standby | - |
| edgion_controller_leader_transitions_total | Counter | controller | none | self | fixed | Total leadership transitions (elected or demoted) — a double-leader bug or flapping election is invisible on the boolean gauge and obvious in this rate | - |
| edgion_controller_reload_client_notifications_total | Counter | controller | none | self | fixed | Client (Gateway) notifications sent for a ConfigSyncServer generation change | - |
| edgion_controller_status_writes_total | Counter | controller | kind, backend (kubernetes/filesystem/etcd), result (success/error) | self | fixed | Status writes attempted by the Controller. Steady state is flat (writes are gated by a status_changed diff) | `{result="error"}` sustained (SSA 409 conflict loop) |
| edgion_controller_status_writes_skipped_total | Counter | controller | kind | self | fixed | Status writes skipped because a semantic diff detected no meaningful change | - |
| edgion_controller_status_conditions_max | Gauge | controller | kind, namespace, name | self | snapshot | Largest single `.status.conditions` array length on a resource, hand-rendered off the resource_abnormal snapshot. Emitted ONLY for resources at/above `metrics.resource_abnormal.status_conditions_threshold` (default 20), so healthy fleets emit nothing — a bloated array is a status-write loop or cross-controller condition pollution | `edgion_controller_status_conditions_max > 0` |
| edgion_controller_unexpected_apply_during_init_total | Counter | controller | kind | self | fixed | Apply events received during the init phase (should stay 0; non-zero indicates watcher-stream ordering anomalies) | - |
| edgion_controller_leader_job_alive | Gauge | controller | leader_job (acme/conf_sync_rotation/credential_manager/dns_resolver) | self | fixed | Leader-job worker liveness: 1 = running, 0 = stopped or dead. Label is `leader_job`, not `job`, to avoid Prometheus's reserved-label rename | `edgion_controller_leader_job_alive == 0 and on(instance) edgion_controller_leader == 1` (`for: 2m`) |
| edgion_metrics_series_rejected_total | Counter | both | none | self | fixed | Over-cap series-registration attempts seen by the runtime cardinality guard. Counts events, not distinct series: in enforce mode every emission of a rejected key increments it (rejected keys are never admitted, so a hot bad label inflates it); in observe mode each distinct over-cap key increments it once | alert on sustained increase |
| edgion_metrics_cardinality_limit_active | Gauge | both | none | self | fixed | 1 while the cardinality cap has been breached (new series dropped/flagged), 0 otherwise | alert == 1 |

## Business

| metric | type | process | labels | category | lifetime | description | alert |
|--------|------|---------|--------|----------|----------|-------------|-------|
| edgion_backend_requests_total | Counter | gateway | configurable (see backend_requests.labels) | business | dynamic | The only per-request outcome counter; gateway-level totals and failures are derived from this via `sum(...)` and the `status` label | `sum(rate(edgion_backend_requests_total{status=~"5xx\|failed"}[5m])) / sum(rate(edgion_backend_requests_total[5m]))` |
| edgion_gateway_request_duration_ms | Histogram | gateway | configurable (see request_duration.labels); disabled by default | business | dynamic | End-to-end request latency — the only histogram on the data plane | `histogram_quantile(0.99, sum(rate(edgion_gateway_request_duration_ms_bucket[5m])) by (le))` |
| edgion_gateway_request_bytes_total | Counter | gateway | none | business | fixed | Total gateway request bytes proxied (downstream → upstream) | - |
| edgion_gateway_response_bytes_total | Counter | gateway | none | business | fixed | Total gateway response bytes proxied (upstream → downstream) | - |
| edgion_gateway_genai_tokens_total | Counter | gateway | direction (input/output), provider (openai/openai_compatible/anthropic), usage_source (reported/estimated) | business | fixed | Optional default-off, process-local GenAI token rate/trend counter; aggregates every dispatched attempt with fixed labels, resets on restart, and never replaces the durable per-attempt access-log ledger | - |
| edgion_mirror_requests_total | Counter | gateway | status (ok/body_abandoned/channel_full/buffer_limit/connect_err/connect_timeout/write_err/timeout) | business | fixed | Request-mirror outcome counter (fire-and-forget; `body_abandoned` means the downstream transport did not reach its real EOS before mirroring stopped, so Edgion cancelled the mirror rather than letting the target accept a partial upload as complete; a request whose transport ended on the header block has a complete empty body, a full chunk queue is `channel_full`, and exhausted per-mirror/process retained-byte admission is `buffer_limit`; no latency histogram — the mirror access log's `et` field carries per-request timing) | - |
| edgion_conn_ip_restriction_events_total | Counter | gateway | result (overlay_missing/overlay_disabled/overlay_type_mismatch/overlay_invalid/fail_closed_profile_missing/denied) | business | fixed | `GlobalConnectionIpRestriction` stream-plugin decision events | - |
| edgion_geoip_denied_total | Counter | gateway | layer (stage1/tcp), reason (deny_country/allow_miss/db_unavailable/lookup_failed/ref_unavailable) | business | fixed | L4 connections denied by a geo rule. Never carries a client address; HTTP denials are deliberately absent here since they already appear in the access log | - |
| edgion_connection_rate_limit_rejected_total | Counter | gateway | dimension (listener/source_ip), stage (local/redis) | business | fixed | Connections rejected by the `ConnectionRateLimit` stream plugin | - |
| edgion_connection_rate_limit_redis_failure_total | Counter | gateway | dimension (listener/source_ip), cause (unavailable/runtime) | business | fixed | Redis failure encountered by the `ConnectionRateLimit` stream plugin, fired regardless of the resulting Allow/Deny outcome so fail-open stays observable | - |
| edgion_downstream_connections_rejected_total | Counter | gateway | reason (global/listener/route) | business | fixed | Connections closed by a connection cap (UDP: each denied new-session admission attempt, i.e. datagram): `global` = `server.max_downstream_connections`, `listener` = Gateway-attached `ConnectionLimit`, `route` = TCPRoute/UDPRoute-attached `ConnectionLimit` | - |
| edgion_circuit_breaker_ejections_total | Counter | gateway | service, class (gateway/application/local_origin) | business | dynamic | Circuit-breaker ejection events, split by which failure class tripped (upstream 5xx vs. local connect/proxy failure) | - |
| edgion_circuit_breaker_ejected_endpoints | Gauge | gateway | service | business | dynamic | Currently ejected endpoint count per service | - |
| edgion_circuit_breaker_rejections_total | Counter | gateway | service | business | dynamic | Requests blocked by an already-ejected backend | - |
| edgion_gateway_circuit_breaker_shed_total | Counter | gateway | service | business | dynamic | Requests shed by the per-endpoint circuit-breaker concurrency cap (`CbPermit::try_acquire` at cap) — distinct from the outlier-detection ejection/rejection metrics above, which trip on backend health rather than in-flight count | - |
| edgion_lb_degrade_selections_total | Counter | gateway | service | business | dynamic | Cumulative selections entering the degrade branch across all Service port sections; retries count separately and selection does not imply successful forwarding. Registered when a Service gains health/outlier policy, including zero-threshold Services; lifecycle changes do not clear totals, but unowned idle series may expire | `sum by (service) (rate(edgion_lb_degrade_selections_total[5m]))` |
| edgion_rr_selector_truncated_total | Counter | gateway | none | business | fixed | RoundRobin selector backend-count truncation (backend count exceeded `u16::MAX`) — an alarm for an extreme configuration anomaly | - |
| edgion_region_route_config_events_total | Counter | gateway | result (setting_reference_unavailable/setting_missing/setting_disabled/setting_type_mismatch/setting_invalid/hash_modulo_invalid) | business | fixed | RegionRoute configuration failures before request mutation; setting_invalid includes consumer/table incompatibility | - |
| edgion_acme_challenge_lookup_total | Counter | gateway | result (hit/expired/domain_mismatch/not_found/empty_host) | business | fixed | ACME HTTP-01 challenge token lookup outcomes | - |

## Cardinality budget

The degrade selection Counter measures events across Service port sections, not
the current health of a Service. A zero rate can mean normal selection, disabled
degrade mode, or no traffic. Health/outlier policy registers a zero-valued series
even when the live threshold is zero, because an in-flight request may retain a
nonzero Service override. Services without either capability do not register a series. A captured in-flight selection can still add an event after Service
deletion; policy updates do not erase history. Live snapshots retain their metric
series across idle scrapes. After the last snapshot releases, the configured idle
timeout can expire the series; recreating it then starts a new total. Process
restart also resets the Counter.
The bundled alert tests a 15-minute increase with a 5-minute pending period; it
reports recent events and may remain active after traffic has recovered.

Known high-cardinality levers, their defaults, and worst-case series counts.

| lever | default | worst-case series | risk |
|-------|---------|-------------------|------|
| `backend_requests.labels.route` | off | `15 x N_route` | high |
| `backend_requests.labels.backend` | off | `15 x N_backend` | high |
| `request_duration.enabled` + labels | off | `buckets x label-product` | high |
| `metrics.genai_tokens.enabled` | off | 12 | bounded |
| `resource_abnormal` (controller) | cap 1000 | <= cap | bounded |
| `idle_timeout` cold eviction | 0 (no-op today) | grows if enabled without guard | medium |
| any `ns`/`name`/`module` label | opt-in | scales with deployment | high |

Runtime safety net: a `CardinalityGuard` (added in a later task) caps distinct
recorder-registered series at `metrics.cardinality_cap` (default 10000);
`edgion_controller_resource_abnormal` is hand-rendered outside the recorder and
relies on its own 1000-entry cap.

OTLP LinkSys health uses a validated `namespace/name` in the existing `link` label.
Several log outputs referencing one resource share one health series and probe.
The gauge reports TCP reachability, not export acknowledgment; endpoint URIs and
resolved addresses are never labels.
