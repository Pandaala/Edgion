---
name: circuit-breaker-local-origin-split
description: Use when reviewing circuit-breaker findings of the form "connect/TLS/transport failures are counted in the same gateway bucket as backend 502/503/504" — implemented as a separate LocalOrigin dimension.
status: fixed
finding_id: resilience-04
closed: 2026-06-21
---

# Circuit Breaker — Local-Origin Failure Dimension Split — IMPLEMENTED

## Conclusion

The passive-outlier-detection failure model now splits **local-origin** failures
(connect failure/timeout, TLS-handshake timeout, transport reset, mid-proxy
read/write error — the data plane never got a usable HTTP response) into their own
`FailureClass::LocalOrigin` dimension with an independent counter
(`EndpointState::local_origin_failures`), an independent threshold
(`consecutiveLocalOriginFailures`), and an independent metric label
(`edgion_circuit_breaker_ejections_total{class="local_origin"}`). This aligns with
Envoy's `splitExternalLocalOriginErrors`.

Re-reports of "connect/proxy failures share the gateway 502/503/504 bucket / same
threshold / same metric" are **not accepted**. Proposals to **merge** the
local-origin bucket back into `Gateway` are also not accepted (that reintroduces the
mis-attribution this finding fixed).

## Core Rationale

**1. What was wrong**

`FailureClass` had only `Gateway` and `Application`. The five connect/proxy-error
call sites (`pg_fail_to_connect`, `pg_error_while_proxy`, `tcp/edgion_tcp`,
`tls/proxy`, `tls/passthrough_proxy`) all recorded `FailureClass::Gateway`, the same
class used for a backend that was online and returned HTTP 502/503/504
(`from_http_status` via `pg_upstream_response_filter`). A backend doing graceful 503
overload-shedding was ejected on the same counter/threshold as a dead port, and
operators could not tell network/data-plane ejections from backend-5xx ejections.

**2. What was implemented**

- `FailureClass::LocalOrigin` variant added; `from_http_status` unchanged (502/504
  still map to `Gateway` — local-origin failures never carry an HTTP status).
- The five connect/proxy call sites now record `FailureClass::LocalOrigin`.
- `consecutiveLocalOriginFailures` config field; when unset it inherits the resolved
  `consecutiveGatewayErrors` in `normalize`, so a config that predates the field
  keeps counting connect/proxy errors at exactly the threshold they used when they
  shared the gateway bucket — **behaviour is byte-for-byte unchanged for old configs.**
- `EndpointState::local_origin_failures` counter, independent eject decision, reset
  on `record_success` and on cooldown recovery alongside the other two counters.
- `record_cb_ejection(service_key, class)` adds a bounded `class` label
  (`gateway`/`application`/`local_origin`) to `edgion_circuit_breaker_ejections_total`.
  The `edgion_circuit_breaker_ejected_endpoints` gauge stays unlabelled by class so
  its +1/-1 balance with `record_cb_recovery` (which has no class context) holds.

**3. Ejection-reason attribution correctness**

Each `record_failure` bumps exactly one counter (by `class`); once any counter
crosses its threshold, `ejected_at` is set and later failures early-return. So the
counter that newly tripped is always the one matching the current call's `class` —
passing `class.metric_label()` to `record_cb_ejection` is exact, not a heuristic.

## Fix Suggestions Not Accepted

- Merging `LocalOrigin` back into `Gateway`.
- Adding a `class` label to the `CB_EJECTED_ENDPOINTS` gauge (would break the
  unlabelled +1/-1 recovery balance — recovery has no class context).
- Adding `interval`-based periodic scanning or `successRate` distribution ejection
  (out of scope — higher cost, marginal benefit until a real misfire/attribution
  complaint exists).

## Re-evaluation Triggers

Re-open only if:

- A new connect/transport failure call site is added that still records `Gateway`.
- Envoy-style `interval` / `successRate` outlier detection is genuinely needed
  (promote to `tasks/todo/` via brainstorming, not a narrow review fix).

## Reference Cases

- Source: 2026-06-21 resilience review (resilience-04).
- Regression: `local_origin_counter_is_independent_of_gateway`,
  `gateway_failures_do_not_trip_local_origin_threshold`,
  `success_resets_local_origin_counter`, `failure_class_metric_labels_are_bounded`
  (store.rs); `local_origin_failures_defaults_to_gateway_errors`,
  `local_origin_failures_explicit_overrides_inheritance`,
  `zero_local_origin_failures_rejected` (config.rs).
- Related: `backends/set-service-config-toctou-race.md` (same `set_service_config`
  change-detection path, now also compares `consecutive_local_origin_failures`).
