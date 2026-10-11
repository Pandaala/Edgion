---
id: ARCH-078
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-229
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Preserve process-owned degraded state across metric idle eviction

Common's system_metrics.rs promises that edgion_system_error_state stays at 1 while its process-owned condition is active. Startup reasons such as skip_tls and integration_testing_mode are marked once and deliberately never cleared. mark_system_error and clear_system_error currently store that state only through a one-shot metrics::gauge! emission; no durable subsystem state is available for the exporter to sample afterward.

The real Gateway installs Common's recorder before marking those reasons. Its RequestDurationConfig defaults idle_timeout_secs to 600, including when the optional duration histogram is disabled. metrics_api::install_global_recorder enables idle eviction for MetricKindMask::ALL. Locked metrics-exporter-prometheus 0.18.3 render iterates gauges and calls metrics-util 0.20.4 Recency::should_store_gauge. Repeated scrapes with unchanged update generation beyond the timeout delete the gauge. Reading it does not renew the update generation. Gateway's scrape wrapper refreshes installed certificate expiry and published LinkSys/LB/Wasm metrics, but never the system-error family.

Consequently, a Gateway explicitly running skip_tls can initially expose system_error_state{component="conf_sync_client",reason="skip_tls"}=1, then cease exposing that sample after an idle interval even though the channel remains plaintext. The integration-testing latch has the same ownership problem. A sustained failure marked once can also disappear while still active. Controller explicitly installs idle_timeout_secs=0 and is not the demonstrated eviction trigger. This is a static source witness with ordinary configured/default inputs; no incident, executed timed scrape or unauthorized enablement is claimed.

Give this fixed, bounded process-state family a canonical lifetime independent of exporter activity. The existing Common system-error owner should retain the last marked/cleared state of recognized bounded component/reason pairs and expose a cold sampling boundary; the existing canonical Gateway scrape can emit that snapshot before render. Keep unseen pairs absent unless deliberately marked or cleared, preserve explicit zero on clear, and coordinate mark/clear versus sampling so a stale sample cannot overwrite a newer clear or mark. Reuse the same mechanism from any shared rendering boundary that needs it rather than creating a second Gateway state owner. A scoped exporter exemption could be an alternative if it preserves other families' cleanup; globally disabling idle eviction or all gauge eviction expands policy unnecessarily. No timer, registry-wide scan, new metric family, peer label or request-path lookup is needed.

Preserve single process/condition ownership and the documented latch/transient distinctions: successful identity reconciliation must never clear the leader rotation reason, healthy peers must not clear another peer's failure, and certificate watcher failure remains latched. Metrics remain observability only; this proposal does not forbid the authorized skip_tls escape hatch, change TLS or test-mode behavior, or duplicate request observability. Retain cardinality admission/no-op semantics and fixed bounded labels. The cardinality guard's own retained handles are a separate lifetime subject and are not rolled into this finding.

Original system-error-state-metric guidance requires continuous state observability and explicitly distinguishes logs from persistent gauge state. The current actual Common tests assert first emission, clear-to-zero, component isolation and identity/rotation ownership using a local recorder without idle eviction. They do not cover loss under the real Gateway policy. Future actual-exporter coverage must mark once, render repeatedly beyond an injected/controlled idle interval, and still expose 1; clearing must expose 0, untouched pairs must remain absent, and unrelated idle series must still expire. Check mark/clear versus concurrent render and the Controller's no-eviction configuration without live sleeps or new production timers.

ARCH-043 shares the exporter trigger but addresses detached writable Counter/Gauge/Histogram handles retained by request owners; its proposed handle registration/pinning cannot recreate this system family's state because this module retains no handle or authoritative snapshot. This finding is the distinct missing process-state lifetime owner and should coordinate at the same canonical rendering seam, not introduce another generic pinning framework. Classification: behavior-defect, P2. Evidence is current source, existing test bodies and locally cached locked dependencies only; no source edits, builds, tests, runtime experiments or network operations.
