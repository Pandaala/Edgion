---
name: installed-certificate-expiry
description: Review conf_sync certificate expiry against installed transport identity, candidate publication, and shared EdgionTls validity metadata.
knowledge_type: regression-guards
---

# Certificate expiry follows the installed identity

## Conclusion and scope

Gateway `edgion_conf_sync_cert_expiry_timestamp_seconds{side="client"}` reports the
certificate configured on the installed conf_sync Channel, not the current contents
of its certificate file. `/metrics` samples preprocessed validity and calls the
existing shared reporter through `GatewayMetrics`. No certificate I/O, PEM parsing,
new metric family, or separate Gateway expiry polling task is involved in a scrape.

`PreparedClient` keeps the Channel and `Option<LeafValidity>` together through
initialization, recovery and TLS reload. `InstalledChannel` publishes them in one
ArcSwap snapshot. Failed, stale or uncommitted candidates cannot advance the gauge.
Explicit plaintext has no validity and no client-expiry series. Initialization
failure remains visible through startup errors and auto-CA retry diagnostics, with
no expiry claimed before a transport is installed.

EdgionTls and ordinary Gateway serving certificates carry the same `LeafValidity`
in their compiled identity. The common classifier preserves RFC 5280 inclusive
bounds and allocation-free handshake checks. Invalid EdgionTls updates that retain
the previous compiled generation retain its validity as well. This change adds no
serving-certificate metric and does not merge the different TLS stores.

## Rationale and evidence

The original `conf-sync-client-expiry-watch-installed-identity` task identified an
observability defect: disk certificate B could have a later expiry even while
rejected or stalled reload kept Channel A. The file-only source parsed B successfully
and could clear A's expiry alert. Existing `cert_reload_invalid` alerting covers
rejection, but cannot prove installation or cover a reload path that never runs.

Authority: [metrics contract](../../../03-coding/observability/01-metrics.md),
[Gateway link architecture](../../../01-architecture/03-controller-gateway-link/02-gateway-side.md),
and [graded rules](../../../00-design-rules.md) for unified mechanisms, secret safety,
atomic related state and Gateway metrics ownership.

Current sources: `edgion-common/src/{cert_util,grpc_tls,tls_validation}.rs`,
`edgion-gateway/src/conf_sync/conf_client/grpc_client.rs`,
`edgion-gateway/src/observe/metrics/{api,registry}.rs`, and the compiled serving
identity consumers in `runtime/matching/tls.rs` and `tls/`.

## Alternatives and limits

Reading Secret or file expiry at scrape time still describes a candidate rather
than what each Gateway has loaded. Parsing certificate material on every scrape is
also unnecessary. Absolute installed expiry remains constant while Prometheus
`time()` advances, so stalled renewal still enters the alert window.

Controller reporting remains hourly from its live server identity; Gateway severity
transition logs now occur on scrapes. Prometheus absence/target-health monitoring is
needed when scrapes do not happen; this metric does not implement monitoring itself.
The accepted file-watcher `(mtime, length)` trigger, existing failover fencing and
publication order, and certificate validation policy remain unchanged. Common client
TLS loading returns validity together with its existing config; the federation
caller discards the metadata and retains its transport behavior.

Client metadata follows tonic's rustls loader: the first `CERTIFICATE` PEM block
supplies validity, even when other PEM labels precede it. PEM decoder failures use
fixed diagnostics rather than input-derived markers. This does not broaden the
shared reload/provision validator's accepted inputs or change Controller metadata
selection; mixed-prefix loading coverage applies to initial client construction.

## Verification and re-evaluation

The new `edgion-gateway/tests/conf_sync_cert_expiry.rs` drives production initialization,
TLS reload and the real in-process metrics router through stalled reload, mismatched
key rejection, successful replacement and vanished files. It also covers missing
auto-CA material and explicit plaintext. Channels stay lazy: this test does not prove
remote mTLS handshakes or filesystem notification delivery.

Existing publication tests now assert pending/cancelled publication and stale-result
metadata boundaries. EdgionTls retention tests assert compiled identity and validity
remain together. Shared validity tests cover inclusive bounds and saturated remaining
time, first-CERTIFICATE selection and sanitized malformed-PEM errors.

Fresh verification on 2026-10-05:

- Common library filters: `cert_util::tests` 6 passed, `tls_validation::` 35 passed,
  `grpc_tls::` 5 passed, `conf_sync::cert_expiry_watch::` 20 passed.
- Gateway library filters (serial): `conf_sync::conf_client::` 30 passed and
  `tls::` 241 passed.
- Gateway integration targets: `conf_sync_cert_expiry`, `conf_sync_payload_client`
  and `conf_sync_failover`, each 1 passed. The existing payload client test uses
  owned fake peers with real mTLS for initial connection, recovery and TLS reload;
  it does not prove the new mixed-prefix fixture over a remote handshake.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Agent docs, SSA force, metric
  inventory, Pingora source policy, Gateway API transform contract, unit-test
  layout, Gateway tracing and diff whitespace guards passed.
- Independent Astra review returned LGTM after first-CERTIFICATE selection,
  sanitized PEM diagnostics and metric-name corrections. An intermediate
  validator change was withdrawn after review identified Controller metadata
  consumers with a different selection policy; that validator remains unchanged.

No full workspace test suite, filesystem-notify scenario or shared
Gateway/Controller end-to-end run is claimed.

Re-evaluate if the Channel identity can mutate independently of its installed
snapshot, scrape sources change, startup begins serving metrics before identity
installation, TLS mode becomes reloadable, or candidate publication gains new paths.

## Resolution references

- `conf-sync-client-expiry-watch-installed-identity` — corrected in this run.
- `D0018` — campaign pointer to the original installed-identity investigation.
