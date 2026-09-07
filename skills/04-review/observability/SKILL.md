---
name: review-observability
description: Review knowledge for observability, grouped by decision type.
---

# observability review knowledge

Read [routing and classification](../knowledge-routing.md) before adding or moving a rule.
Search these pages by behavior, symbol, or finding ID; revalidate historical evidence
before applying a decision to current code.

## Mandatory design

- [Config-sync batch logs: `info!` is count-only, key detail is `debug!`](mandatory-design/conf-sync-batch-info-count-only.md)
- [DSL `Vm::execute` runtime-error tracing — DESIGN-TRADEOFF (DECLINED)](mandatory-design/dsl-vm-execute-tracing-declined.md)
- [`global_plugins_resolve` Dangling Refs Are Not Logged (Fixed)](mandatory-design/global-plugins-resolve-no-tracing.md)
- [Observability Review Rules](mandatory-design/observability-review-rule.md)
- [`PluginRuntime` `add_*` Builders Are Tracing-Free (Fixed)](mandatory-design/plugin-runtime-builder-no-tracing.md)
- [TCP `check_stream_plugins` Fail-Close Is Not Logged via Tracing (Fixed)](mandatory-design/tcp-stream-plugin-no-tracing.md)
- [`run_tls_route_plugins` Stage 2 → Stage 1 Compat Fallback Is Not Logged via Tracing](mandatory-design/tls-route-fallback-no-tracing.md)

## Intentional design

- [`BandwidthLimit::create` invalid-rate path — NO RUNTIME LOG, DOC CLARIFIED](intentional-design/bandwidth-limit-invalid-rate-no-tracing.md)
- [`ServerCache::apply_change` per-change INFO is intentionally kept at INFO](intentional-design/cache-server-apply-change-info-level.md)
- [Generic Watch decode diagnostics and deferred recovery](intentional-design/conf-sync-watch-parse-error-metric-only.md)
- [DSL runtime-error line numbers in access log — DESIGN-TRADEOFF (DECLINED)](intentional-design/dsl-error-line-number-declined.md)
- [observe F-011 — Cold-metric OnceLock boilerplate is not extracted into a macro](intentional-design/metrics-registry-cold-oncelock-pattern-not-extracted.md)
- [observe F-010 — Protocol logger OnceLock pattern is not extracted](intentional-design/protocol-logger-oncelock-pattern-not-extracted.md)
- [route-03 (per-try-timeout deep review) — retry Prometheus counter declined by design](intentional-design/retry-counter-metric-declined.md)

## Regression guards

- [observe F-003 (access_log) — all-senders-unhealthy path increments the dropped counter](regression-guards/access-log-all-senders-unhealthy-metric.md)
- [observe F-B01 (access_log) — disabled mode installs a no-op logger instead of leaving OnceLock empty](regression-guards/access-log-disabled-init-noop.md)
- [observe F-006 — AccessLogStore TTL cleanup on read paths, no background timer](regression-guards/access-log-store-ttl-read-path-cleanup.md)
- [ConfigClient apply_resource_change data_preview Log Leak — FIXED (Iron Rule 1)](regression-guards/config-client-data-preview-log-safety.md)
- [observe F-B03 — `create_async_logger` takes typed `LogType`, not `&str`](regression-guards/create-async-logger-typed-log-type.md)
- [`genai.model` is bounded but may be client-controlled](regression-guards/genai-model-is-bounded-but-client-controlled.md)
- [`GET /geoip` admin endpoint removed — metrics and the access log already cover it — FIXED](regression-guards/geoip-admin-endpoint-removed-metrics-and-access-log-cover-it.md)
- [GeoIP lookup failures follow failOpen — FIXED](regression-guards/geoip-lookup-failures-follow-fail-open.md)
- [GeoIP has no global `enabled` switch; the access log never forces a lookup — FIXED](regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md)
- [GeoIP updater success test uses a fixture-scale cap — FIXED](regression-guards/geoip-updater-success-test-fixture-scale-cap.md)
- [EgressResolver blocked-address tracing is removed — FIXED](regression-guards/gw-plugin-outbound-01-egress-resolver-tracing-removed.md)
- [link-sys F-020 (local_file) — background write errors are surfaced, `current_size` no longer drifts](regression-guards/local-file-write-error-surfaced.md)
- [OIDC `JwksProvider` dual-use tracing — FIXED (4 deleted, 1 relocated to the cold caller)](regression-guards/oidc-jwks-provider-tracing-relocated.md)
- [OIDC outbound SAN-mismatch hot-path `tracing::warn!` removed — FIXED](regression-guards/oidc-outbound-san-mismatch-tracing-removed.md)
- [Pingora HTTP/1 Header Logs Have an Error Floor — FIXED](regression-guards/pingora-h1-header-logs-have-an-error-floor.md)
- [`RealIp::run_request` Trusted-Proxy-Match Tracing Is Removed (Fixed)](regression-guards/real-ip-run-request-tracing-removed.md)
- [Response body size finalization is idempotent — FIXED](regression-guards/response-body-size-idempotent-finalization.md)
- [`StreamPluginStore::partial_update` per-key `tracing::debug!` removed](regression-guards/stream-plugin-store-partial-update-per-key-debug-removed.md)
- [`edgion_system_error_state` Gauge: Registry of System-Level Degraded States](regression-guards/system-error-state-metric.md)
- [path-norm-nonhttp-routes F-B02 — UNSAFE_UPSTREAM_ADDRESS_LOG centralized + TCP path emits the literal](regression-guards/unsafe-upstream-address-log-centralized.md)
- [Wasm request-path tracing (guest message + callout URL) removed — FIXED](regression-guards/wasm-request-path-tracing-removed.md)

## Review evidence

- [`extract_key_from_path` / `extract_key_from_etcd_key` None Branches Are Defensive Fallbacks](review-evidence/conf-center-extract-key-none-defensive.md)
