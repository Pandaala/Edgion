---
name: tls-route-stage-isolation
description: Use when reviewing TLSRoute stream-plugin stage selection, explicit empty configuration, or hot reload.
knowledge_type: regression-guards
---

# TLSRoute executes only its declared route-stage plugins

## Conclusion and scope

Both Terminate and Passthrough call `routes/tls/mod.rs::run_tls_route_plugins`
before an upstream connection. That function executes only
`CompiledStreamPlugins.tls_route_plugin_runtime`, built from
`EdgionStreamPluginsSpec.tls_route_plugins` (`spec.tlsRoutePlugins` on the wire).
`TlsRouteUnit.stream_plugin_store_key` resolves the route annotation to the
current immutable compiled resource at the start of each connection.

| Referenced resource declaration | TLSRoute result |
|---|---|
| Missing resource | Fail closed |
| Stage-2 absent/null, raw Stage-1 nonempty | Fail closed, require `tlsRoutePlugins` |
| Explicit Stage-2 empty list | No-op; never execute Stage-1 |
| Stage-2 all disabled or Bypass | No-op; never execute Stage-1 |
| Enabled Stage-2 Block | Deny |
| Nonempty active Stage-2 | Run it, first deny wins |
| Both stages absent/empty | No-op |
| No resource annotation | No-op |

Canonical IP parsing still precedes evaluation when a resource is referenced;
an unparseable direct or remote IP fails closed even for a no-op resource.
The wrong-stage check uses raw Stage-1 list presence (`has_plugins()`), not its
compiled runtime. Disabled or Bypass Stage-1 entries do not make an invalid
TLSRoute attachment valid. Declare Stage-2 intent explicitly instead. This
is an attachment rule, not compilation or validation of disabled plugins.

## Root cause and authority

The old dispatcher chose a stage using runtime emptiness. Adding one active
Stage-2 entry stopped Stage-1 evaluation; disabling or bypassing the last
Stage-2 entry reactivated Stage-1. Removing that branch alone would also have
turned existing Stage-1 deny-only fixtures into no-policy allows.

The [stream plugin architecture](../../../01-architecture/02-gateway/13-stream-plugin-dev.md#stage-2-tlsroute)
identified this as pre-release compatibility debt. The fix removes dispatch
and the GeoIP-specific filter, rejects undeclared Stage-2 intent, and migrates
TLSRoute IP fixtures to `tlsRoutePlugins`. TLS listener GeoIP coverage instead
uses a dedicated Gateway attachment and `stage1` metrics. GeoIP and
ConnectionRateLimit do not become Stage-2 plugins.

## Lifecycle and regression boundaries

`compile_stream_plugins` preserves raw schema and builds both runtimes;
`Option::is_none` serialization preserves `Some([])` through conf_sync.
`StreamPluginStore` publishes one atomic generation. New connections observe
the latest publication; established connections are not re-authorized.
No additional cache, lock, runtime scan, or compatibility adapter is needed.

The regression matrix in `routes/tls/tests.rs` exercises both TLS modes,
serialized configuration generations, explicit empty/disabled/Bypass/Block,
missing resources, invalid canonical addresses, and delete/recreate. Its
global-store mutations are serial: unique keys alone cannot make multiple
`partial_update` writers safe because production relies on conf-sync serialization.

Existing TLSRoute/StreamPlugins deny/allow fixtures are migrated in local and
Kubernetes trees. The canonical `TlsRoutePluginsReload` scenario checks both
TLS modes through real connections and shared Controller/Gateway topology.
Unreachable listeners or timeouts must not count as policy denials.

## Verification and re-evaluation

Current verification:

- `cargo test -p edgion-gateway --lib routes::tls::tests -- --test-threads=1`:
  8 passed, including the serialized both-mode stage/hot-update matrix.
- `cargo test -p edgion-tests --bin test_client`: 21 registration/harness checks passed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed; Clippy reports existing
  workspace warnings.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py`: passed.
- Independent runtime, integration, and final diff reviews completed. Review
  corrections included valid CIDR lists in the dynamic fixtures, wrong-stage
  examples, runtime ownership guidance, and obsolete GeoIP metric guidance.
- Canonical `prepare.sh` passed. Using `start_all_with_conf.sh --suites
  TLSRoute/StreamPlugins,TLSRoute/Passthrough,TLSRoute/GeoRestriction` and the
  built `test_client` in one process session: StreamPlugins 4/4,
  Passthrough 3/3, and `TlsRoutePluginsReload` passed. An initial attempt
  separated startup from execution; the tool session ended its background
  services and probes correctly failed on connection refusal. Running startup
  and execution in the same session resolved this environment failure.
- On 2026-09-10, with the standard loopback aliases available, the canonical
  default integration sweep passed: 237 report entries passed, zero failed or
  timed out; the sole skipped entry was Build because `prepare.sh` had already
  passed. Report: `tmp/integration_testing/testing_20260910_154454/report.log`.
  This includes TLSRoute GeoRestriction, StreamPlugins, Passthrough, and
  `TlsRoutePluginsReload`, completing the remaining network verification.
  The selected task is closed. Kubernetes execution and `--full-test` slow
  scenarios were not run; mirrored fixture contracts have source/test coverage.

Re-evaluate changes to Stage-2 optional-field serialization, dispatch placement,
plugin policy compilation, or connection-generation ownership against this matrix.
Observability is governed separately by
[TLSRoute plugin access logs](../../observability/mandatory-design/tls-route-plugin-access-log.md).

## Resolution references

- `remove-tlsroute-stage1-fallback-coherently`: coordinated removal of pre-release fallback debt.
- `D0115`: original deferred campaign evidence; no runtime fix was claimed by that deferral.
