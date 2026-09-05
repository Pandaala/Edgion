# Edgion Alerting — Severity Model & Alert Catalog

This page is the authority for **how Edgion alerts are graded and routed**. The rules
themselves live in the opt-in bundle
([`cicd/observability/alerts/edgion-rules.yml`](../../../../cicd/observability/alerts/edgion-rules.yml)
for bare Prometheus, `cicd/observability/k8s/prometheusrule.yaml` for the Operator).
Metric definitions are in [metrics-inventory.md](metrics-inventory.md).

## Severity model

Edgion uses the three standard Prometheus `severity` values — **not** a P0/P1/P2
vocabulary — mapped onto the operational division of labor:

| `severity` | meaning | Alertmanager route | pages? |
|------------|---------|--------------------|--------|
| `critical` | **PAGE** — wake someone now | `pager` receiver | yes |
| `warning`  | **TICKET** — handle in business hours | `ticket` receiver | no |
| `info`     | **log-only / board** — visibility, not action | `low-priority` receiver | no |

`info` alerts exist so a signal is *routable and queryable*, but they never page.
Dashboards own "see everything"; alerts are a **curated** set of global-anomaly /
must-not-happen signals — most emitted metrics are intentionally *not* alerted and are
watched on the boards instead.

Routing templates: `cicd/observability/alertmanager/` (bare + Operator CR). Both route
purely on the `severity` label, so re-grading a rule re-routes it automatically.

## critical (PAGE)

| alert | signal | `for:` |
|-------|--------|--------|
| EdgionConfigSyncFrozen | Gateway config-sync link down, serving stale config | 10m |
| EdgionLeaderJobDead | a leader-only job (acme / conf_sync_rotation / credential_manager / dns_resolver) died on the leader | 2m |
| EdgionCertExpiryCritical | in-service conf_sync cert expires in < 7 days (the PAGE band) | 1h |
| EdgionCertRotationFailed | `system_error_state{reason=~"cert_rotation_failed\|cert_identity_sync_failed"}` — rotation pipeline failing | 5m |

The conf_sync cert-rotation reason grading is canonical in the
[controller config guide](../../../../skills/02-features/02-config/00-controller-config.md)
(its **Alerting** subsection). The two cert rules above are derived from it; this page
does not restate the per-reason reasoning.

## warning (TICKET)

| alert | signal | `for:` |
|-------|--------|--------|
| EdgionCertExpirySoon | conf_sync cert expires in < 30 days (the TICKET band — distinct from the 7d PAGE band, not a duplicate) | 1h |
| EdgionResourceAbnormal | a resource has a failing status condition | 15m |
| EdgionSchemaDrift | `config_event_parse_errors` — Controller/Gateway schema drift | 5m |
| EdgionFipsHostModuleMismatch | FIPS host running a non-FIPS binary | 5m |
| EdgionOutboundTlsSanMismatch | outbound TLS SAN-pin rejection (cert rotation / misconfig) | 5m |
| EdgionIpDenyEntriesSkipped | deny-CIDR entries skipped — blacklist hole (fail-open) | 5m |
| EdgionLinkSysUnhealthy | a LinkSys provider is unhealthy | 10m |
| EdgionWasmGuestFailing | wasm guest trapping or failing closed | 5m |
| EdgionLbDegradeMode | load-balancer degrade mode engaged | 5m |
| EdgionUnexpectedApplyDuringInit | apply during init — watcher-stream anomaly (should stay 0) | 5m |
| EdgionConnRateLimitRedisFailure | L4 rate-limit Redis failure (fail-open visibility) | 5m |
| EdgionStatusConditionsHigh | a single resource's `.status.conditions` array is bloated past the threshold — a status-write loop (single-controller death-loop) or cross-controller condition pollution | 10m |
| EdgionStatusWriteErrorsSustained | status writes for a kind keep failing — likely an SSA 409 conflict loop between competing field managers / multiple controllers | 15m |
| EdgionSystemErrorStateLatched | `system_error_state` for the operator-actionable config-suspect / watch-failed / security-posture reasons | 10m |
| EdgionCardinalityGuardActive | cardinality cap breached; new series dropped/flagged | 5m |
| EdgionConfigConflict | a config-aggregation conflict silently fell back to a default (e.g. conflicting outbound CA defaults → System) | 5m |
| EdgionOutboundInjectedHeaderSkipped | a configured outbound-injected header was silently dropped (invalid name/value) | 5m |

`EdgionSystemErrorStateLatched` deliberately **excludes** `confsync_disconnected` (owned
by `EdgionConfigSyncFrozen`) and `integration_testing_mode` (a benign deliberate latch),
so there is no double-fire and no noise from intentional test/debug states.

`EdgionStatusConditionsHigh` measures the **content**, not the write rate: the
controller's `resource_abnormal` snapshot loop already scans every resource's
`.status.conditions`, so it also records the **largest single conditions-array length**
and emits `edgion_controller_status_conditions_max{kind,namespace,name} = <length>`
**only** for resources at/above the threshold (`metrics.resource_abnormal.status_conditions_threshold`,
default 20). Healthy fleets emit zero series, so the alert can fire on presence
(`> 0`) and still carry the exact resource identity — no log triage needed. A normal
resource has a small, bounded conditions array (a few condition *types*); a loop or
cross-controller pollution makes one array grow without bound, which is the signal.
The threshold is chosen per fleet in controller config, not in the alert rule, so the
series set stays bounded. (Measuring the single-array max — not the flattened total —
avoids false positives on a legitimately multi-listener / multi-parent resource whose
arrays are each individually small.)

## info (log-only / board; never pages)

| alert | signal | `for:` |
|-------|--------|--------|
| EdgionMetricSeriesHigh | an Edgion metric's series count is high (cardinality trend) | 15m |
| EdgionOutboundEgressBlocked | outbound egress blocked by the SSRF policy | 5m |
| EdgionOutboundTemplateFallback | a templated outbound field silently fell back to its default | 5m |

## Note on metric categories

The inventory tags each metric `self` or `business`, but that split is documentation
only (there is no runtime `category` label). A few `business` metrics
(`ip_deny_entries_skipped`, `connection_rate_limit_redis_failure`, `lb_degrade_mode`) are
alerted here as operational health — the category tag is not an alerting boundary.
