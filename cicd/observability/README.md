# Edgion Observability Bundle

Opt-in Grafana dashboards + Prometheus alert rules + Alertmanager routing for Edgion.
Nothing here is wired into any default deployment — pick what you need. Metric
definitions:
[../../docs/en/ops-guide/observability/metrics-inventory.md](../../docs/en/ops-guide/observability/metrics-inventory.md).
Alert severity model + full alert catalog:
[../../docs/en/ops-guide/observability/alerting.md](../../docs/en/ops-guide/observability/alerting.md).

## Non-Kubernetes (docker / bare Grafana + Prometheus + Alertmanager)
1. Copy `dashboards/*.json` to `/var/lib/grafana/dashboards/edgion/`.
2. Copy `provisioning/dashboards.yaml` into Grafana's provisioning directory
   (e.g. `/etc/grafana/provisioning/dashboards/`).
3. Add `alerts/edgion-rules.yml` to Prometheus `rule_files:`.
4. Merge `alertmanager/alertmanager.yml` into your Alertmanager config and replace the
   placeholder receivers.
5. Point Prometheus at each process's `/metrics` (Gateway `:12090`, Controller `:12190`).

## Kubernetes (Prometheus Operator + Grafana sidecar)
1. `kubectl apply -f k8s/servicemonitor.yaml` — adjust `selector`/`namespaceSelector`
   to match your Edgion Service labels and namespaces.
2. `kubectl apply -f k8s/prometheusrule.yaml` — adjust the `release` label to match
   your Prometheus Operator's `ruleSelector`.
3. `kubectl apply -f alertmanager/k8s/alertmanagerconfig.yaml` — adjust the label
   selector your Alertmanager uses to pick up `AlertmanagerConfig` CRs, and replace the
   placeholder receivers.
4. `kubectl apply -f k8s/dashboards-configmap.yaml` — the Grafana sidecar auto-imports
   any ConfigMap labeled `grafana_dashboard: "1"`.

## Dashboards
- `edgion-self` — gateway/controller health: config-sync, leader/leader-job, reload,
  cert expiry, wasm, abnormal resources, and the self-cardinality overview.
- `edgion-business` — traffic SLIs: request/error rate, latency (enable the
  `request_duration` histogram first), rate-limit, circuit breaker, mirror, bytes.
- `edgion-federation` — multi-cluster federation. **Requires EdgionCenter** (separate
  repo); aggregator/watch-error panels show "No data" on a Controller-only cluster.

## Alerting
`alerts/edgion-rules.yml` (bare Prometheus) and `k8s/prometheusrule.yaml` (Operator CR)
carry the same rules, graded into the three standard Prometheus severities:

- `critical` = **PAGE** (wake someone) — config-sync frozen, leader-job dead, cert
  expiry < 7d, cert-rotation failing.
- `warning` = **TICKET** (business hours) — cert expiry < 30d, abnormal resources,
  schema drift, FIPS host/module mismatch, TLS SAN mismatch, blacklist holes, LinkSys
  unhealthy, wasm guest failing, LB degrade, and the other operator-actionable sentinels.
- `info` = **log-only / board** (never pages) — high metric cardinality, cardinality
  guard active, SSRF egress blocks, outbound template fallback.

`alertmanager/` routes by the `severity` label:
- `alertmanager/alertmanager.yml` — bare Alertmanager routing template.
- `alertmanager/k8s/alertmanagerconfig.yaml` — Prometheus Operator `AlertmanagerConfig`.

Both ship non-functional placeholder receivers you must fill in. The full per-alert
catalog, `for:` guards, and rationale live in
[../../docs/en/ops-guide/observability/alerting.md](../../docs/en/ops-guide/observability/alerting.md);
the conf_sync cert-rotation reason grading stays canonical in the
[controller config guide](../../skills/02-features/02-config/00-controller-config.md).

## Metric categories
Every Edgion metric is classified `self` (gateway/controller own health) or `business`
(tenant traffic outcomes) in the inventory. The split is documentation only — there is
no runtime `category` label. Dashboards mirror the split (self vs business boards). A few
`business` metrics (`ip_deny_entries_skipped`, `connection_rate_limit_redis_failure`,
`lb_degrade_mode`) are alerted as operational health — the category label is documentation,
not an alerting boundary.

## Deploy decision (is it shipped with Edgion?)
This bundle is NOT applied by any Edgion install path — it is a starting point tenants
opt into and adapt (namespaces, selectors, Prometheus release labels, retention). Copy
what fits; treat the JSON/YAML as templates, not turnkey manifests.
