# Edgion Observability Bundle

Opt-in Grafana dashboards + Prometheus alert rules for Edgion. Nothing here is wired
into any default deployment — pick what you need. Metric definitions:
[../../../docs/en/ops-guide/observability/metrics-inventory.md](../../../docs/en/ops-guide/observability/metrics-inventory.md).

## Non-Kubernetes (docker / bare Grafana + Prometheus)
1. Copy `dashboards/*.json` to `/var/lib/grafana/dashboards/edgion/`.
2. Copy `provisioning/dashboards.yaml` into Grafana's provisioning directory
   (e.g. `/etc/grafana/provisioning/dashboards/`).
3. Add `alerts/edgion-rules.yml` to Prometheus `rule_files:`.
4. Point Prometheus at each process's `/metrics` (Gateway `:12090`, Controller `:12190`).

## Kubernetes (Prometheus Operator + Grafana sidecar)
1. `kubectl apply -f k8s/servicemonitor.yaml` — adjust `selector`/`namespaceSelector`
   to match your Edgion Service labels and namespaces.
2. `kubectl apply -f k8s/prometheusrule.yaml` — adjust the `release` label to match
   your Prometheus Operator's `ruleSelector`.
3. `kubectl apply -f k8s/dashboards-configmap.yaml` — the Grafana sidecar auto-imports
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
carry the same rules: config-sync frozen, leader-job dead, cert expiry < 7d, abnormal
resources, high metric cardinality, and cardinality-guard active. See the
[metrics inventory](../../../docs/en/ops-guide/observability/metrics-inventory.md) and
[metrics ops guide](../../../docs/en/ops-guide/observability/metrics.md) for the full
signal catalog.

## Metric categories
Every Edgion metric is classified `self` (gateway/controller own health) or `business`
(tenant traffic outcomes) in the inventory. The split is documentation only — there is
no runtime `category` label. Dashboards mirror the split (self vs business boards).

## Deploy decision (is it shipped with Edgion?)
This bundle is NOT applied by any Edgion install path — it is a starting point tenants
opt into and adapt (namespaces, selectors, Prometheus release labels, retention). Copy
what fits; treat the JSON/YAML as templates, not turnkey manifests.
