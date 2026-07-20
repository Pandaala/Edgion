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

## Self

| metric | type | process | labels | category | description | alert |
|--------|------|---------|--------|----------|-------------|-------|
| edgion_ctx_created_total | Counter | gateway | none | self | Total request contexts created (requests received) | - |
| edgion_ctx_active | Gauge | gateway | none | self | Currently active in-flight request contexts | - |
| edgion_access_log_dropped_total | Counter | gateway | none | self | Dropped HTTP access-log entries (backpressure/failure on the log-shipping path) | - |
| edgion_ssl_log_dropped_total | Counter | gateway | none | self | Dropped SSL/TLS-termination log entries | - |
| edgion_tcp_log_dropped_total | Counter | gateway | none | self | Dropped TCP stream access-log entries | - |
| edgion_tls_log_dropped_total | Counter | gateway | none | self | Dropped TLS-route access-log entries | - |
| edgion_udp_log_dropped_total | Counter | gateway | none | self | Dropped UDP stream access-log entries | - |
| edgion_access_log_backfill_ingested_total | Counter | gateway | none | self | Access-log documents successfully re-ingested into Elasticsearch from the local fallback file during a backfill run | - |
| edgion_access_log_backfill_failed_total | Counter | gateway | none | self | Access-log lines that failed to backfill into Elasticsearch and are kept on disk for a later retry | - |
| edgion_config_reload_signals_total | Counter | gateway | none | self | Reload signals received by the Gateway from the Controller over conf_sync | - |
| edgion_config_relist_total | Counter | gateway | none | self | Config relist operations performed by the Gateway's conf_sync client | - |
| edgion_config_event_parse_errors_total | Counter | gateway | kind | self | Watch-event JSON parse failures per resource kind — the primary signal for Controller/Gateway schema drift | - |
| edgion_link_sys_es_bulk_channel_full_total | Counter | gateway | none | self | Elasticsearch producer→bulk-loop channel was full at submit time; raise `bulk.channelSize` | - |
| edgion_link_sys_health | Gauge | gateway | link | self | LinkSys provider health: 1 = healthy, 0 = unhealthy, per provider | - |
| edgion_link_sys_health_flips_total | Counter | gateway | link, direction (recovered/degraded) | self | LinkSys provider health flip counter — exposes sub-scrape flapping the level gauge cannot | - |
| edgion_ldap_auth_errors_total | Counter | gateway | kind (unreachable/timeout/bind_fail/tls_san_mismatch) | self | LDAP auth plugin error counter by failure kind | - |
| edgion_gateway_outbound_tls_san_mismatch_total | Counter | gateway | plugin (ldap_auth/openid_connect) | self | Outbound TLS SAN-pinning rejection — signals a misconfigured `validation.subjectAltNames` or a rotated peer certificate | - |
| edgion_webhook_resolve_errors_total | Counter | gateway | reason (not_found/unhealthy/rate_limited/status_error/call_failed/authority_rejected) | self | Webhook key-resolver per-resolve failure counter by reason | - |
| edgion_outbound_egress_blocked_total | Counter | gateway | reason (loopback/link_local/private/unspecified) | self | Outbound address rejected by the SSRF egress policy, by reason | - |
| edgion_gateway_upstream_authority_rejected_total | Counter | gateway | reason (render_empty/invalid_label) | self | Request-time upstream authority the Gateway refused to use (empty template render, or the rendered value is not a valid DNS label) | - |
| edgion_outbound_success_predicate_fail_total | Counter | gateway | none | self | Outbound response accepted at the status layer but classified as failure by a `success.body` predicate ("200 OK but error in the body") | - |
| edgion_outbound_template_fallback_total | Counter | gateway | field (timeout_ms/allow_degradation) | self | A templated outbound field failed to render and silently fell back to its static default | - |
| edgion_gateway_fips_module_enabled | Gauge | gateway | none | self | 1 = the BoringSSL FIPS-validated crypto module is linked and enabled in this binary; the only validated-module compliance signal | `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0` |
| edgion_gateway_fips_host_enabled | Gauge | gateway | none | self | 1 = the host kernel reports FIPS mode (`/proc/sys/crypto/fips_enabled`) | `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0` |
| edgion_gateway_fips_host_detect_error | Gauge | gateway | none | self | 1 = the host FIPS flag exists but could not be read (masked `/proc`); both the guardrail and `fips_policy=auto` are inert in this state | - |
| edgion_gateway_fips_policy_active | Gauge | gateway | validated (true/false) | self | 1 = the approved FIPS cipher/curve profile is in effect on downstream listeners; `validated="false"` means approved suites over an unvalidated module | - |
| edgion_wasm_module_load_total | Counter | gateway | module, result (ok/error) | self | Wasm module load attempt outcome | - |
| edgion_wasm_module_compile_duration_ms | Histogram | gateway | module | self | Wall-clock duration of one Wasm `Module::new` compilation | - |
| edgion_wasm_module_evicted_total | Counter | gateway | module | self | Wasm module-cache entry eviction (refcount reached zero and the grace period elapsed) | - |
| edgion_wasm_instance_instantiate_total | Counter | gateway | module | self | Fresh Wasm VM instantiation (instance pool miss) | - |
| edgion_wasm_instance_discard_total | Counter | gateway | module, reason (poisoned/over_capacity) | self | Wasm VM discarded instead of returned to the pool | - |
| edgion_wasm_pool_size | Gauge | gateway | module | self | Current pooled Wasm instance count for a module | - |
| edgion_wasm_hook_duration_ms | Histogram | gateway | module, phase (request_headers/response_headers/response_body/http_call_response/tick) | self | Duration of one Wasm guest hook invocation | - |
| edgion_wasm_trap_total | Counter | gateway | module, phase | self | Wasm guest trap (or protocol violation that poisons the lease) | - |
| edgion_wasm_fail_mode_total | Counter | gateway | module, reason, mode (open/close) | self | Fail-mode decision taken by the Wasm plugin host | - |
| edgion_wasm_callout_total | Counter | gateway | module, result (ok/error/denied/over_limit/tick_config_conflict) | self | Guest-dispatched HTTP callout outcome | - |
| edgion_wasm_modules_loaded | Gauge | gateway | none | self | Number of compiled Wasm modules currently held in the global cache (sizing input for pooling-allocator capacity) | - |
| edgion_wasm_shared_data_bytes | Gauge | gateway | module | self | Current shared-data byte total for a Wasm module | - |
| edgion_wasm_body_pause_unsupported_total | Counter | gateway | module | self | A Wasm guest returned `Action::Pause` from the response-body hook, which this host cannot honor (per-chunk window, not a cumulative buffer) — the guest silently observes only the current chunk | alert on this counter (no fixed threshold) |
| edgion_gateway_plugin_policy_blocked_total | Counter | gateway | plugin | self | A request was rejected because its plugin chain contains a Block-stamped plugin under controller policy | - |
| edgion_gateway_plugin_policy_bypassed_total | Counter | gateway | plugin | self | A request passed through a chain with a Bypass-stamped plugin skipped under controller policy | - |
| edgion_conf_sync_cert_expiry_timestamp_seconds | Gauge | both | side (server/client) | self | The in-service conf_sync certificate's `notAfter` (Unix seconds), independent of whatever rotation machinery is supposed to be renewing it | `edgion_conf_sync_cert_expiry_timestamp_seconds - time() < 30 * 86400` |
| edgion_system_error_state | Gauge | both | component, reason | self | `1` while a known degraded operating condition is active on this process (see `edgion-common/src/observe/system_metrics.rs` for the full bounded `component`/`reason` sets); some reasons latch until restart, others clear on the next success | latching: `== 1`; transient: sustained `== 1` with a `for:` guard |
| edgion_fed_connections_active | Gauge | controller | role (center/controller) | self | Currently active federation connections from the emitting process's point of view (Center side lives in the separate EdgionCenter repo) | - |
| edgion_fed_connection_events_total | Counter | controller | role, event (connected/disconnected/reload) | self | Federation session lifecycle events (this repo emits from the Controller only) | - |
| edgion_fed_connection_duration_seconds_last | Gauge | controller | role | self | Last observed federation session duration in seconds, recorded at disconnect | - |
| edgion_fed_watch_events_total | Counter | controller | kind, direction (sent/recv) | self | Watch events flowing across the federation link | - |
| edgion_fed_ready_gate_wait_seconds_last | Gauge | controller | none | self | Last observed ready-gate wait in seconds (Controller side) | - |
| edgion_fed_rbac_denied_total | Counter | controller | verb, kind, source (center/cli_token/unknown) | self | RBAC denials on the Controller admin-API federation path | - |
| edgion_fed_kill_switch_state | Gauge | controller | none | self | Federation kill-switch state; 1 = enabled, 0 = disabled. Emitted on every supervisor start/stop transition | - |
| edgion_controller_resource_abnormal | Gauge | controller | kind, namespace, name, err, warn | self | One series per resource with at least one failing status condition; `err`/`warn` are JSON arrays of the triggering condition types, capped at 1000 entries per snapshot | `edgion_controller_resource_abnormal{err!=""}` |
| edgion_controller_init_sync_duration_seconds_last | Gauge | controller | kind | self | Duration of the last initial sync, per resource kind | - |
| edgion_controller_active | Gauge | controller | none | self | Number of active resource controllers | - |
| edgion_controller_resources_watched | Gauge | controller | kind | self | Resources currently watched, per kind | - |
| edgion_controller_leader | Gauge | controller | none | self | Leader election status: 1 = leader, 0 = standby | - |
| edgion_controller_leader_transitions_total | Counter | controller | none | self | Total leadership transitions (elected or demoted) — a double-leader bug or flapping election is invisible on the boolean gauge and obvious in this rate | - |
| edgion_controller_reload_total | Counter | controller | none | self | Total reload operations triggered | - |
| edgion_controller_reload_duration_seconds_last | Gauge | controller | none | self | Duration of the last reload | - |
| edgion_controller_reload_client_notifications_total | Counter | controller | none | self | Client (Gateway) notifications sent for a reload | - |
| edgion_controller_status_writes_total | Counter | controller | kind, backend (kubernetes/filesystem), result (success/error) | self | Status writes attempted by the Controller | - |
| edgion_controller_status_writes_skipped_total | Counter | controller | kind | self | Status writes skipped because a semantic diff detected no meaningful change | - |
| edgion_controller_unexpected_apply_during_init_total | Counter | controller | kind | self | Apply events received during the init phase (should stay 0; non-zero indicates watcher-stream ordering anomalies) | - |
| edgion_controller_leader_job_alive | Gauge | controller | leader_job (acme/conf_sync_rotation/dns_resolver) | self | Leader-job worker liveness: 1 = running, 0 = stopped or dead. Label is `leader_job`, not `job`, to avoid Prometheus's reserved-label rename | `edgion_controller_leader_job_alive == 0 and on(instance) edgion_controller_leader == 1` (`for: 2m`) |
| edgion_metrics_series_rejected_total | Counter | both | none | self | Over-cap series-registration attempts seen by the runtime cardinality guard. Counts events, not distinct series: in enforce mode every emission of a rejected key increments it (rejected keys are never admitted, so a hot bad label inflates it); in observe mode each distinct over-cap key increments it once | alert on sustained increase |
| edgion_metrics_cardinality_limit_active | Gauge | both | none | self | 1 while the cardinality cap has been breached (new series dropped/flagged), 0 otherwise | alert == 1 |

## Business

| metric | type | process | labels | category | description | alert |
|--------|------|---------|--------|----------|-------------|-------|
| edgion_backend_requests_total | Counter | gateway | configurable (see backend_requests.labels) | business | The only per-request outcome counter; gateway-level totals and failures are derived from this via `sum(...)` and the `status` label | `sum(rate(edgion_backend_requests_total{status=~"5xx\|failed"}[5m])) / sum(rate(edgion_backend_requests_total[5m]))` |
| edgion_gateway_request_duration_ms | Histogram | gateway | configurable (see request_duration.labels); disabled by default | business | End-to-end request latency — the only histogram on the data plane | `histogram_quantile(0.99, sum(rate(edgion_gateway_request_duration_ms_bucket[5m])) by (le))` |
| edgion_gateway_request_bytes_total | Counter | gateway | none | business | Total gateway request bytes proxied (downstream → upstream) | - |
| edgion_gateway_response_bytes_total | Counter | gateway | none | business | Total gateway response bytes proxied (upstream → downstream) | - |
| edgion_mirror_requests_total | Counter | gateway | status (ok/channel_full/connect_err/connect_timeout/write_err/timeout) | business | Request-mirror outcome counter (fire-and-forget; no latency histogram — the mirror access log's `et` field carries per-request timing) | - |
| edgion_conn_ip_restriction_events_total | Counter | gateway | result (overlay_missing/overlay_disabled/overlay_type_mismatch/overlay_invalid/fail_closed_profile_missing/denied) | business | `GlobalConnectionIpRestriction` stream-plugin decision events | - |
| edgion_ip_deny_entries_skipped_total | Counter | gateway | scope (http/stream_l4/tls_route) | business | Inline `deny` CIDR entries skipped by partial-good compilation — a hole in a blacklist (under-block / fail-open direction) | - |
| edgion_connection_rate_limit_rejected_total | Counter | gateway | dimension (listener/source_ip), stage (local/redis) | business | Connections rejected by the `ConnectionRateLimit` stream plugin | - |
| edgion_connection_rate_limit_redis_failure_total | Counter | gateway | dimension (listener/source_ip), cause (unavailable/runtime) | business | Redis failure encountered by the `ConnectionRateLimit` stream plugin, fired regardless of the resulting Allow/Deny outcome so fail-open stays observable | - |
| edgion_circuit_breaker_ejections_total | Counter | gateway | service, class (gateway/application/local_origin) | business | Circuit-breaker ejection events, split by which failure class tripped (upstream 5xx vs. local connect/proxy failure) | - |
| edgion_circuit_breaker_ejected_endpoints | Gauge | gateway | service | business | Currently ejected endpoint count per service | - |
| edgion_circuit_breaker_rejections_total | Counter | gateway | service | business | Requests blocked by an already-ejected backend | - |
| edgion_lb_panic_mode | Gauge | gateway | service | business | 1 when load-balancer panic mode is engaged for the named service (healthy fraction below the configured threshold), 0 when normal | - |
| edgion_rr_selector_truncated_total | Counter | gateway | none | business | RoundRobin selector backend-count truncation (backend count exceeded `u16::MAX`) — an alarm for an extreme configuration anomaly | - |
| edgion_region_route_overlay_events_total | Counter | gateway | result (overlay_missing/overlay_disabled/overlay_type_mismatch/overlay_invalid) | business | `RegionRoute` overlay resolution events (fail-to-base outcomes when an EdgionConfigData overlay is missing, disabled, mismatched, or invalid) | - |
| edgion_acme_challenge_lookup_total | Counter | gateway | result (hit/expired/domain_mismatch/not_found/empty_host) | business | ACME HTTP-01 challenge token lookup outcomes | - |

## Cardinality budget

Known high-cardinality levers, their defaults, and worst-case series counts.

| lever | default | worst-case series | risk |
|-------|---------|-------------------|------|
| `backend_requests.labels.route` | off | `15 x N_route` | high |
| `backend_requests.labels.backend` | off | `15 x N_backend` | high |
| `request_duration.enabled` + labels | off | `buckets x label-product` | high |
| `resource_abnormal` (controller) | cap 1000 | <= cap | bounded |
| `idle_timeout` cold eviction | 0 (no-op today) | grows if enabled without guard | medium |
| any `ns`/`name`/`module` label | opt-in | scales with deployment | high |

Runtime safety net: a `CardinalityGuard` (added in a later task) caps distinct
recorder-registered series at `metrics.cardinality_cap` (default 10000);
`edgion_controller_resource_abnormal` is hand-rendered outside the recorder and
relies on its own 1000-entry cap.
