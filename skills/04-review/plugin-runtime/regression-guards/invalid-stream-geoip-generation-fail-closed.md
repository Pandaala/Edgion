---
name: invalid-stream-geoip-generation-fail-closed
description: Use when reviewing invalid L4 GeoIpLocation generation publication, TCP denial, or valid replacement recovery.
knowledge_type: regression-guards
---

# Invalid stream GeoIP generations fail closed

## Conclusion and scope

An enabled, non-bypassed `EdgionStreamPlugins` GeoIpLocation entry with empty
`denyRuleGroups` and `allowRuleGroups` is invalid at L4. Controller
`Accepted=False` reports that invalidity; the Gateway separately compiles an
always-deny gate at the front of `StreamPluginRuntime.plugins`. A valid replacement
rebuilds the runtime and restores its configured policy.

The TCPRoute GeoRestriction case `parse_only_is_rejected_at_l4` exercises a valid
CIDR-only allow baseline, the invalid status and live TCP closure, and a valid
replacement with status and echo recovery. It restores the baseline on error as
well, and reports restoration failures instead of silently ignoring them.

## Rationale and evidence

- [Shared configuration validation](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/geoip_location.rs):
  `GeoIpLocationConfig::check_fields(true)` rejects parse-only configuration.
  `fail_open` governs database/lookup failure for valid rules, not invalid config.
- [Controller handler](../../../../edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs):
  `preparse` uses that validation and `update_status` reports `Accepted=False`.
- [Stream runtime](../../../../edgion-gateway/src/plugins/stream/stream_plugin_runtime.rs):
  constructor errors install `FailClosedStreamPlugin` at index zero. The existing
  `invalid_geoip_config_installs_an_always_deny_gate` and
  `invalid_geoip_config_cannot_be_undone_and_policy_block_stays_first` unit tests
  cover gate retention and policy-Block precedence.
- [Stream store](../../../../edgion-gateway/src/plugins/stream/stream_plugin_store.rs) and
  [publisher](../../../../edgion-gateway/src/plugins/publication/publisher.rs):
  full and incremental source updates flow through atomic scoped publication;
  `StreamPluginGeneration` builds the demanded executable views.
- [TCP lifecycle](../../../../edgion-gateway/src/routes/tcp/edgion_tcp.rs):
  `check_stream_plugins` consumes the runtime before upstream forwarding.
- [Integration case](../../../../edgion-tests/src/client/suites/tcp_route/geo_restriction/geo_restriction.rs):
  D0101 previously checked only status, leaving live enforcement and recovery
  unverified even though B0152/B0156 had already corrected the runtime.

## Alternatives and limits

Status alone does not establish Gateway activation. The case permits stale
baseline echoes while conf_sync converges, but requires a post-connect closure
within eight seconds. Bind/connect failures and read timeouts are inconclusive,
never successful denials. Baseline and recovery probes distinguish the intended
transition from a persistently unavailable listener/backend. The check phase has
a 40-second bound, restoration a separate 30-second bound, and the enclosing case
allows 75 seconds so ordinary failures cannot cancel cleanup prematurely.

This is client-observed TCP forwarding evidence, not a backend-side connection
audit or proof for every stream lifecycle. It does not assert a GeoIP denial
metric: the invalid-config gate runs before the GeoIP plugin. The
[lookup-failure guard](../../observability/regression-guards/geoip-lookup-failures-follow-fail-open.md)
covers a different failure class and cannot substitute for generation testing.
The [stream validation guard](../../ip-restriction/regression-guards/stream-preparse-uses-plugin-validation.md)
describes shared validation, not live publication verification.

## Verification and re-evaluation

Verified locally on macOS on 2026-09-09:

- `./edgion-tests/integration/scripts/integration/run_integration.sh -r TCPRoute -i GeoRestriction`:
  all nine cases passed, zero infrastructure errors. The target case observed
  invalid status, a post-connect reset, and 21 recovery echo bytes. Runner
  preflights and final metrics-label, unknown-field, requeue, and cache-residue
  checks also passed.
- `cargo test -p edgion-gateway --lib invalid_geoip_config_`: both existing
  invalid-configuration gate tests passed (2 passed, 0 failed).
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed; Clippy reported existing
  warnings outside the changed case.
- `python3 cicd/checks/validate_agent_docs.py`,
  `python3 cicd/checks/check_ssa_force.py`,
  `python3 cicd/checks/check_metrics_inventory.py`, and
  `python3 cicd/checks/check_pingora_source_policy.py`: passed.

These are fresh results, separate from historical B0152/B0156 evidence.
Re-evaluate when stream compilation, publication, route attachment, probe
classification, or case timeout behavior changes. K8s uses the existing suite's
real-pod-peer and nonmatching CIDR baseline; this local run does not establish
K8s execution. Error cleanup branches were reviewed, not fault-injected live.

## Resolution references

- D0101 / `verify-invalid-l4-geoip-generations-fail-closed`: confirmed integration
  coverage gap, closed on 2026-09-09 after verification and independent review;
  test-only correction, with production gate and unit tests retained.
