# 指标清单

> 每个 `edgion_*` 指标的权威目录。路径与英文版对称，供中文文档内链解析。
> CI 护栏 `cicd/checks/check_metrics_inventory.py` **只校验**
> [`docs/en/ops-guide/observability/metrics-inventory.md`](../../../en/ops-guide/observability/metrics-inventory.md)
> 与代码发点之间的双向漂移；本页表格字段与英文权威清单保持一致。
>
> **Category** 为 `self`（Edgion 自身健康/基础设施）或 `business`（租户流量经 Gateway
> 的业务结果）。该列仅用于文档分类，**不是**运行时 Prometheus 标签。设计说明见
> `docs/superpowers/specs/2026-07-20-metrics-observability-hardening-design.md` §2。
>
> **Process** 标明发点进程：`gateway`、`controller` 或 `both`（两侧共享代码路径，例如联邦连接生命周期）。
> 仅 Center 发点的联邦指标（如 `edgion_fed_watch_list_total`、`edgion_fed_aggregator_*`）不在此列出 —
> Center 已拆至独立的 EdgionCenter 仓库，不在本护栏范围内。
>
> AI Gateway 相关业务指标见下方 Business 表中的 `edgion_gateway_genai_tokens_total`，
> 以及 Cardinality budget 中的 `metrics.genai_tokens.enabled`。

通用 Watch 解码失败时，Gateway 每个失败响应输出一条 error，记录首个失败的
`kind`、`ns`、`name`、`resourceVersion`、`syncVersion` 和安全错误摘要。
缺失或无效的身份字段省略；顶层 JSON 失败使用响应的同步版本。日志不包含资源正文
或解码器错误中的原始字段值。首个解码失败计入指标并结束当前 Watch，剩余事件不再
处理或计数。Gateway 沿用已有退避流程重新 List，再开启 Watch。等待有效新快照期间，
保留已经成功应用的配置和 readiness。该信号不代表恢复成功：持续的结构不匹配也可能
导致 List 失败，请核对 Controller/Gateway 构建版本并排查对应资源。
EdgionBackend 保留独立的撤销策略。

## Self（自身）

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
| edgion_config_event_parse_errors_total | Counter | gateway | kind | self | 按资源 kind 统计 Watch 解码失败；通用 Watch 首次失败计一次并重新 List，不再解码剩余事件 | - |
| edgion_link_sys_es_bulk_channel_full_total | Counter | gateway | none | self | Elasticsearch producer→bulk-loop channel was full at submit time; raise `bulk.channelSize` | - |
| edgion_link_sys_health | Gauge | gateway | link | self | LinkSys provider health: 1 = healthy, 0 = unhealthy, per provider | - |
| edgion_link_sys_health_flips_total | Counter | gateway | link, direction (recovered/degraded) | self | LinkSys provider health flip counter — exposes sub-scrape flapping the level gauge cannot | - |
| edgion_ldap_auth_errors_total | Counter | gateway | kind (unreachable/timeout/bind_fail/tls_san_mismatch) | self | LDAP auth plugin error counter by failure kind | - |
| edgion_gateway_outbound_tls_san_mismatch_total | Counter | gateway | plugin (ldap_auth/openid_connect) | self | Outbound TLS SAN-pinning rejection — signals a misconfigured `validation.subjectAltNames` or a rotated peer certificate | - |
| edgion_webhook_resolve_errors_total | Counter | gateway | reason (not_found/unhealthy/rate_limited/status_error/call_failed/authority_rejected) | self | Webhook key-resolver per-resolve failure counter by reason | - |
| edgion_outbound_egress_blocked_total | Counter | gateway | reason (loopback/link_local/private/unspecified) | self | Outbound address rejected by the SSRF egress policy, by reason | - |
| edgion_gateway_upstream_authority_rejected_total | Counter | gateway | reason (render_empty/invalid_label) | self | Request-time upstream authority the Gateway refused to use (empty template render, or the rendered value is not a valid DNS label) | - |
| edgion_outbound_success_predicate_fail_total | Counter | gateway | none | self | Outbound response accepted at the status layer but classified as failure by a `success.body` predicate ("200 OK but error in the body") | - |
| edgion_outbound_template_fallback_total | Counter | gateway | field (timeout_ms) | self | A templated outbound field failed to render and silently fell back to its static default | - |
| edgion_config_conflict_total | Counter | gateway | field (constant set: outbound_ca, outbound_client_identity) | self | A config-aggregation conflict selected a conservative result: conflicting outbound CA defaults fall back to System, while conflicting outbound client identities make inherited TLS client construction fail closed. Zero-event sentinel | alert on `increase(...) > 0` |
| edgion_outbound_injected_header_skipped_total | Counter | gateway | none | self | A caller-injected outbound header (e.g. X-Forwarded-* from ForwardAuth) was dropped for an invalid name/value — the operator's configured header silently no-ops. Zero-event sentinel | alert on `increase(...) > 0` |
| edgion_gateway_fips_module_enabled | Gauge | gateway | none | self | 1 = BoringSSL reports FIPS mode in this binary; this does not establish CMVP certificate scope or build provenance | `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0` |
| edgion_gateway_fips_host_enabled | Gauge | gateway | none | self | 1 = the host kernel reports FIPS mode (`/proc/sys/crypto/fips_enabled`) | `edgion_gateway_fips_host_enabled == 1 and edgion_gateway_fips_module_enabled == 0` |
| edgion_gateway_fips_host_detect_error | Gauge | gateway | none | self | 1 = the host FIPS flag exists but could not be read (masked `/proc`); both the guardrail and `fips_policy=auto` are inert in this state | - |
| edgion_gateway_fips_policy_active | Gauge | gateway | module_enabled (true/false) | self | 1 = the approved FIPS cipher/curve profile is in effect on downstream listeners; the label records the BoringSSL FIPS-mode signal | - |
| edgion_wasm_module_load_total | Counter | gateway | module, result (ok/error) | self | Wasm module load attempt outcome | - |
| edgion_wasm_module_compile_duration_ms | Histogram | gateway | module | self | Wall-clock duration of one Wasm `Module::new` compilation | - |
| edgion_wasm_module_evicted_total | Counter | gateway | module | self | Wasm module-cache entry eviction (refcount reached zero and the grace period elapsed) | - |
| edgion_wasm_instance_instantiate_total | Counter | gateway | module | self | Fresh Wasm VM instantiation (instance pool miss) | - |
| edgion_wasm_instance_discard_total | Counter | gateway | module, reason (poisoned/over_capacity) | self | Wasm VM discarded instead of returned to the pool | - |
| edgion_wasm_pool_size | Gauge | gateway | module | self | Current pooled Wasm instance count for a module | - |
| edgion_wasm_hook_duration_ms | Histogram | gateway | module, phase (request_headers/request_body/response_headers/response_body/http_call_response/tick) | self | Duration of one Wasm guest hook invocation | - |
| edgion_wasm_trap_total | Counter | gateway | module, phase | self | Wasm guest trap (or protocol violation that poisons the lease) | - |
| edgion_wasm_fail_mode_total | Counter | gateway | module, reason, mode (open/close) | self | Fail-mode decision taken by the Wasm plugin host | - |
| edgion_wasm_callout_total | Counter | gateway | module, result (ok/error/denied/over_limit/tick_config_conflict) | self | Guest-dispatched HTTP callout outcome | - |
| edgion_wasm_modules_loaded | Gauge | gateway | none | self | Number of compiled Wasm modules currently held in the global cache (sizing input for pooling-allocator capacity) | - |
| edgion_wasm_shared_data_bytes | Gauge | gateway | module | self | Current shared-data byte total for a Wasm module | - |
| edgion_wasm_body_pause_unsupported_total | Counter | gateway | module | self | A Wasm guest returned `Action::Pause` from the response-body hook, which this host cannot honor (per-chunk window, not a cumulative buffer) — the guest silently observes only the current chunk | alert on this counter (no fixed threshold) |
| edgion_gateway_plugin_policy_blocked_total | Counter | gateway | plugin | self | A request was rejected because its plugin chain contains a Block-stamped plugin under controller policy | - |
| edgion_gateway_plugin_policy_bypassed_total | Counter | gateway | plugin | self | A request passed through a chain with a Bypass-stamped plugin skipped under controller policy | - |
| edgion_geoip_db_loaded | Gauge | gateway | none | self | 1 when a GeoIP database is loaded, 0 otherwise — the alerting signal for the dangerous silent failure where geo rules are configured but the security control is not running | alert on `edgion_geoip_db_loaded == 0` while geo rules are configured |
| edgion_geoip_db_build_epoch_seconds | Gauge | gateway | none | self | Build timestamp of the loaded GeoIP database | alert on staleness |
| edgion_geoip_update_failed_total | Counter | gateway | reason (fetch/encoding/size_cap/stall/validation/rename/disk/deadline) | self | GeoIP database update failures by class (background updater task only) | - |
| edgion_geoip_lookup_panic_total | Counter | gateway | none | self | A per-request MMDB lookup inside `GeoIpProvider::lookup` panicked and was reported as `Failed`. Distinct from `edgion_geoip_update_failed_total`, which is about the background update cycle. Decode-without-panic failures share that outcome but do not increment this counter | - |
| edgion_gateway_retry_budget_denied_total | Counter | gateway | none | business | Retries refused by a `retryConstraint` budget | - |
| edgion_conf_sync_cert_expiry_timestamp_seconds | Gauge | both | side (server/client) | self | The in-service conf_sync certificate's `notAfter` (Unix seconds), independent of whatever rotation machinery is supposed to be renewing it | `edgion_conf_sync_cert_expiry_timestamp_seconds - time() < 30 * 86400` |
| edgion_system_error_state | Gauge | both | component, reason | self | `1` while a known degraded operating condition is active on this process (see `edgion-common/src/observe/system_metrics.rs` for the full bounded `component`/`reason` sets); some reasons latch until restart, others clear on the next success | latching: `== 1`; transient: sustained `== 1` with a `for:` guard |
| edgion_fed_connections_active | Gauge | controller | role (center/controller) | self | Currently active federation connections from the emitting process's point of view (Center side lives in the separate EdgionCenter repo) | - |
| edgion_fed_connection_events_total | Counter | controller | role, event (connected/disconnected/reload) | self | Federation session lifecycle events (this repo emits from the Controller only) | - |
| edgion_fed_connection_duration_seconds_last | Gauge | controller | role | self | Last observed federation session duration in seconds, recorded at disconnect | - |
| edgion_fed_watch_events_total | Counter | controller | kind, direction (sent/recv) | self | Watch events flowing across the federation link | - |
| edgion_fed_ready_gate_wait_seconds_last | Gauge | controller | none | self | Last observed ready-gate wait in seconds (Controller side) | - |
| edgion_fed_rbac_denied_total | Counter | controller | verb, kind, source (center/cli_token/unknown) | self | RBAC denials on the Controller admin-API federation path | - |
| edgion_fed_kill_switch_state | Gauge | controller | none | self | Federation kill-switch state — the operator's `center.enabled` intent only; 1 = enabled, 0 = disabled. Emitted on every supervisor apply | - |
| edgion_fed_client_running | Gauge | controller | none | self | Whether the federation client is actually running in this process; 1 = running, 0 = not running. `kill_switch_state == 1 and client_running == 0` is the normal steady state of a non-leader replica (only the leader may hold the federation session) | `edgion_fed_kill_switch_state == 1 and max(edgion_fed_client_running) == 0` (enabled but no replica is federating) |
| edgion_controller_resource_abnormal | Gauge | controller | kind, namespace, name, err, warn | self | One series per resource with at least one failing status condition; `err`/`warn` are JSON arrays of the triggering condition types, capped at 1000 entries per snapshot | `edgion_controller_resource_abnormal{err!=""}` |
| edgion_controller_unknown_config_fields | Gauge | controller | kind | self | Number of `EdgionPlugins`, `EdgionStreamPlugins`, `EdgionConfigData`, or `LinkSys` resources whose submitted document contained a key the typed schema does not recognize, with one series per kind (advisory only, never blocks). Detailed bounded findings remain available through status, logs, and `GET /api/v1/diagnostics/unknown-fields`. Driven by a live scan of `spec.unknown_fields`, so — unlike `resource_abnormal` — it is populated in FileSystem/etcd mode too, not Kubernetes-mode only | `edgion_controller_unknown_config_fields > 0` |
| edgion_controller_init_sync_duration_seconds_last | Gauge | controller | kind | self | Duration of the last initial sync, per resource kind | - |
| edgion_controller_active | Gauge | controller | none | self | Number of active resource controllers | - |
| edgion_controller_resources_watched | Gauge | controller | kind | self | Resources currently watched, per kind | - |
| edgion_controller_leader | Gauge | controller | none | self | Leader election status: 1 = leader, 0 = standby | - |
| edgion_controller_leader_transitions_total | Counter | controller | none | self | Total leadership transitions (elected or demoted) — a double-leader bug or flapping election is invisible on the boolean gauge and obvious in this rate | - |
| edgion_controller_reload_total | Counter | controller | none | self | Total reload operations triggered | - |
| edgion_controller_reload_duration_seconds_last | Gauge | controller | none | self | Duration of the last reload | - |
| edgion_controller_reload_client_notifications_total | Counter | controller | none | self | Client (Gateway) notifications sent for a reload | - |
| edgion_controller_status_writes_total | Counter | controller | kind, backend (kubernetes/filesystem), result (success/error) | self | Status writes attempted by the Controller. Steady state is flat (writes are gated by a status_changed diff) | `{result="error"}` sustained (SSA 409 conflict loop) |
| edgion_controller_status_writes_skipped_total | Counter | controller | kind | self | Status writes skipped because a semantic diff detected no meaningful change | - |
| edgion_controller_status_conditions_max | Gauge | controller | kind, namespace, name | self | Largest single `.status.conditions` array length on a resource, hand-rendered off the resource_abnormal snapshot. Emitted ONLY for resources at/above `metrics.resource_abnormal.status_conditions_threshold` (default 20), so healthy fleets emit nothing — a bloated array is a status-write loop or cross-controller condition pollution | `edgion_controller_status_conditions_max > 0` |
| edgion_controller_unexpected_apply_during_init_total | Counter | controller | kind | self | Apply events received during the init phase (should stay 0; non-zero indicates watcher-stream ordering anomalies) | - |
| edgion_controller_leader_job_alive | Gauge | controller | leader_job (acme/conf_sync_rotation/credential_manager/dns_resolver) | self | Leader-job worker liveness: 1 = running, 0 = stopped or dead. Label is `leader_job`, not `job`, to avoid Prometheus's reserved-label rename | `edgion_controller_leader_job_alive == 0 and on(instance) edgion_controller_leader == 1` (`for: 2m`) |
| edgion_metrics_series_rejected_total | Counter | both | none | self | Over-cap series-registration attempts seen by the runtime cardinality guard. Counts events, not distinct series: in enforce mode every emission of a rejected key increments it (rejected keys are never admitted, so a hot bad label inflates it); in observe mode each distinct over-cap key increments it once | alert on sustained increase |
| edgion_metrics_cardinality_limit_active | Gauge | both | none | self | 1 while the cardinality cap has been breached (new series dropped/flagged), 0 otherwise | alert == 1 |

## Business（业务）

| metric | type | process | labels | category | description | alert |
|--------|------|---------|--------|----------|-------------|-------|
| edgion_backend_requests_total | Counter | gateway | configurable (see backend_requests.labels) | business | The only per-request outcome counter; gateway-level totals and failures are derived from this via `sum(...)` and the `status` label | `sum(rate(edgion_backend_requests_total{status=~"5xx\|failed"}[5m])) / sum(rate(edgion_backend_requests_total[5m]))` |
| edgion_gateway_request_duration_ms | Histogram | gateway | configurable (see request_duration.labels); disabled by default | business | End-to-end request latency — the only histogram on the data plane | `histogram_quantile(0.99, sum(rate(edgion_gateway_request_duration_ms_bucket[5m])) by (le))` |
| edgion_gateway_request_bytes_total | Counter | gateway | none | business | Total gateway request bytes proxied (downstream → upstream) | - |
| edgion_gateway_response_bytes_total | Counter | gateway | none | business | Total gateway response bytes proxied (upstream → downstream) | - |
| edgion_gateway_genai_tokens_total | Counter | gateway | direction (input/output), provider (openai/openai_compatible/anthropic), usage_source (reported/estimated) | business | Optional default-off, process-local GenAI token rate/trend counter; aggregates every dispatched attempt with fixed labels, resets on restart, and never replaces the durable per-attempt access-log ledger | - |
| edgion_mirror_requests_total | Counter | gateway | status (ok/body_abandoned/channel_full/connect_err/connect_timeout/write_err/timeout) | business | Request-mirror outcome counter (fire-and-forget; `body_abandoned` means the downstream transport did not reach its real EOS before mirroring stopped, so Edgion cancelled the mirror rather than letting the target accept a partial upload as complete; a request whose transport ended on the header block has a complete empty body, and a full mirror buffer is the distinct `channel_full`; no latency histogram — the mirror access log's `et` field carries per-request timing) | - |
| edgion_conn_ip_restriction_events_total | Counter | gateway | result (overlay_missing/overlay_disabled/overlay_type_mismatch/overlay_invalid/fail_closed_profile_missing/denied) | business | `GlobalConnectionIpRestriction` stream-plugin decision events | - |
| edgion_geoip_denied_total | Counter | gateway | layer (stage1/tcp/tls), reason (deny_country/allow_miss/db_unavailable/lookup_failed) | business | L4 connections denied by a geo rule. Never carries a client address; HTTP denials are deliberately absent here since they already appear in the access log | - |
| edgion_ip_deny_entries_skipped_total | Counter | gateway | scope (http/stream_l4/tls_route) | business | Inline `deny` CIDR entries skipped by partial-good compilation — a hole in a blacklist (under-block / fail-open direction) | - |
| edgion_connection_rate_limit_rejected_total | Counter | gateway | dimension (listener/source_ip), stage (local/redis) | business | Connections rejected by the `ConnectionRateLimit` stream plugin | - |
| edgion_connection_rate_limit_redis_failure_total | Counter | gateway | dimension (listener/source_ip), cause (unavailable/runtime) | business | Redis failure encountered by the `ConnectionRateLimit` stream plugin, fired regardless of the resulting Allow/Deny outcome so fail-open stays observable | - |
| edgion_circuit_breaker_ejections_total | Counter | gateway | service, class (gateway/application/local_origin) | business | Circuit-breaker ejection events, split by which failure class tripped (upstream 5xx vs. local connect/proxy failure) | - |
| edgion_circuit_breaker_ejected_endpoints | Gauge | gateway | service | business | Currently ejected endpoint count per service | - |
| edgion_circuit_breaker_rejections_total | Counter | gateway | service | business | Requests blocked by an already-ejected backend | - |
| edgion_gateway_circuit_breaker_shed_total | Counter | gateway | service | business | Requests shed by the per-endpoint circuit-breaker concurrency cap (`CbPermit::try_acquire` at cap) — distinct from the outlier-detection ejection/rejection metrics above, which trip on backend health rather than in-flight count | - |
| edgion_lb_degrade_selections_total | Counter | gateway | service | business | Cumulative selections entering the degrade branch across all Service port sections; retries count separately and selection does not imply successful forwarding. Registered when a Service gains degrade capability; lifecycle changes do not clear totals, but unowned idle series may expire | `sum by (service) (rate(edgion_lb_degrade_selections_total[5m]))` |
| edgion_rr_selector_truncated_total | Counter | gateway | none | business | RoundRobin selector backend-count truncation (backend count exceeded `u16::MAX`) — an alarm for an extreme configuration anomaly | - |
| edgion_region_route_overlay_events_total | Counter | gateway | result (overlay_missing/overlay_disabled/overlay_type_mismatch/overlay_invalid) | business | `RegionRoute` overlay resolution events (fail-to-base outcomes when an EdgionConfigData overlay is missing, disabled, mismatched, or invalid) | - |
| edgion_acme_challenge_lookup_total | Counter | gateway | result (hit/expired/domain_mismatch/not_found/empty_host) | business | ACME HTTP-01 challenge token lookup outcomes | - |

## 基数预算

降级选路 Counter 累计同一 Service 各端口的选路事件，不表示 Service 当前是否健康。
速率为零可能表示正常选路、降级已禁用或没有流量。从未具备降级能力的 Service
不会创建对应序列。已经捕获快照的选路可以在 Service 删除后继续计数，策略更新不清空历史；
存活快照会在采集时保活指标。最后一个快照释放后，序列可按配置的空闲超时淘汰，
此后重建会从新计数开始；进程重启也会重置 Counter。
附带告警检测最近 15 分钟的增量，并要求条件持续 5 分钟，
因此流量恢复后仍可能暂时保持告警。

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

运行时安全网：`CardinalityGuard`（后续任务落地）将 recorder 注册的不同序列数上限设为
`metrics.cardinality_cap`（默认 10000）；`edgion_controller_resource_abnormal` 在 recorder
之外手工渲染，并依赖自身的 1000 条上限。
