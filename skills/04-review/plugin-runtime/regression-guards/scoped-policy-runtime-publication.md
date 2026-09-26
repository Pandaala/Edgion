---
name: scoped-policy-runtime-publication
description: Review GatewayClass policy publication, retained DSL/WAF candidates, shared entry ownership, and captured HTTP/stream admission scopes.
knowledge_type: regression-guards
---

# Scoped plugin policy governs every published executable view

## Conclusion and scope

`EdgionGatewayConfig.spec.pluginPolicy`, resolved through GatewayClass
`parametersRef`, is the sole execution-policy source. The Controller validates
source resources and transports resolved data; it neither stamps entries nor
decides a shared plugin's execution policy. The removed Controller startup key
is rejected without an alias or fallback.

The canonical Gateway `PluginPublisher` builds immutable class scopes from typed
source, attachment and binding inputs. HTTP requests capture the selected scope;
global, Gateway, route and delayed backend references use that same scope.
A backend ExtensionRef executes only on first use of the selected backend; a
restricted entry on an unselected candidate does not preempt backend selection.
Stream connection and TCP/UDP/TLS-route admissions use their corresponding
published binding views. Existing admitted requests/sessions keep their views.

## Authority and implementation

The user authorized this migration on 2026-09-11. Its authority and lifecycle
matrix were recorded in the now-closed design and implementation tasks.
The [September 9 per-reference decision](../superseded/request-plugin-reference-generation-boundary.md)
is historical; this bounded scope ownership is a newly authorized decision.

Relevant source is `edgion-gateway/src/plugins/publication/{publisher,views,inputs}.rs`,
HTTP/stream `plugin_generation.rs`, and runtime `entry_bundle.rs`. Demand is the
union of allowed slots in active scopes. One source generation owns each entry's
constructor result and companion filters; policy views share permitted instances.
An entry denied in every scope is not constructed. Unneeded slots and unreferenced
source generations are released; in-flight Arcs may extend their lifetime.

## Invalid-generation regression boundary

D0095's old whole-resource retention could publish the previous runtime after
compiling a new Block/Bypass with unrelated invalid DSL/WAF. Both delta updates
and full sync could therefore restore obsolete execution policy.

The publisher evaluates current policy against both the latest source and any
retained executable candidate. Latest-source Block wins, including a newly
declared entry absent from the candidate. Restrictions on entries present only
in the candidate still apply. Block precedes invalid-config/body/condition work;
Bypass excludes construction and execution. Retention never overrides the
existing cross-namespace mirror or non-retainable validation guards.

A relaxation while the latest source is invalid cannot reconstruct a previously
excluded candidate slot. Without a compatible valid candidate, publish the
current generic 500 gate. Missing/invalid referenced policy makes admission
unavailable; no parametersRef uses the explicit default scope. Shared listeners
require equivalent connection views before ownership is known; later HTTP routing
cannot borrow another Gateway's already-executed early policy.

AI rule admission is computed on publication using the shared pure validator and
Controller-resolved backend facts. Block precedes the AI gate. Bypassed AiProxy
cannot turn an AI target into ordinary upstream forwarding or drop the matched
rule into a fallback route. A mismatched route generation fails closed.

## Verification and limits

Migration checkpoint evidence on 2026-09-11:

- Original reproduction: four expected failures across DSL/WAF and delta/full sync.
- `cargo test -p edgion-gateway --lib plugins::http::plugin_store:: --offline`:
  36 passed, including current/new/retained-only Block, Bypass, removal, mirror
  boundaries, in-flight instances and concurrent publication.
- The freshly built Gateway unit binary passed 37 runtime and nine HTTP handler tests.
- Publisher tests: 11 passed; typed input tests: six passed. Early HTTP tests:
  nine passed; scoped resolution: eight passed; TLS: 62 passed; TCP: 22 passed.
  Stream: 86 passed, one existing ignored; context: seven passed; request filter:
  13 passed. Final UDP rerun: 34 passed.
- Controller HTTPRoute extraction: 38 passed; shared pure AI validation: six passed.
  Removed-key/config, HTTP/stream source validation and status-metric tests passed.

The local shared `PluginPolicyDynamic` lifecycle passed, including unchanged
Controller/Gateway PID and start time, followed by `cache_residue --mode fail`
(exit 0; wipe-all 24 resources, zero errors). Workspace all-target check and
Clippy passed; documentation, SSA, metrics, Pingora source and Gateway tracing
guards passed. Independent code and knowledge review returned LGTM.
Live Kubernetes tests passed for one managed Class: Block/Bypass at all HTTP
stages, status changes, API-server field preservation, EGC deletion/recreation,
and class parametersRef rebinding. Controller/Gateway Pod UID and restart counts
remained unchanged. The Kubernetes cache-residue gate passed with validator_rc=0.
At the time of D0095 verification, the Kubernetes `gateway_class` startup filter
permitted only one class, so that historical evidence excluded the shared
two-class Kubernetes scenario. Its local two-class run passed. The subsequent
`kubernetes-gateway-class-allow-list` expansion removes that startup restriction;
its Kubernetes verification is separate from D0095's historical results.
The dynamic scenario needs a freshly preloaded shared topology after its final
cleanup because of a separately recorded listener-recreation defect. Kubernetes
prepare also encounters a separately recorded EdgionTls structural schema rejection;
the test copy removes that one TLS declaration and preserves the EGC CRD exactly.
The supplemental KeyGet cardinality assertion failed independently during that
historical verification and was not included in those passing-check claims. The
later `traceId`/CRD correction is recorded in the
[EdgionPlugins closed-enum guard](../../resources/regression-guards/edgion-plugins-crd-type-enum-drift.md).
Do not infer cross-kind transactions across separately arriving inputs, fleet-wide
atomicity, revocation of already admitted traffic, or snapshots of mutable external
dependencies. Gateway traffic diagnostics remain in canonical access/plugin logs
and existing bounded metrics; publication adds no hot-path tracing or per-chunk log.

Re-evaluate on new attachment consumers, constructor ownership changes, retention
categories, policy availability rules, or request/session capture boundaries.

On 2026-09-12, the focused current-HEAD regression
`plugins::publication::tests::current_policy_wins_over_invalid_dsl_and_waf_for_delta_full_and_event_order`
passed (one passed, zero failed). This fresh result covers DSL/WAF, delta/full,
and policy/source event order; the broader counts above remain historical
migration-checkpoint evidence.

## Processed-resource test fixtures

Fixtures representing Controller-published Gateways and Routes must include
`spec.currentStatus`. Use `crate::test_current_status::from_value` or `processed`
at fixture construction. A Gateway without the current listener status produces
no serving listeners or policy scope; a Route without current owned acceptance
produces no accepted parents. Complete API `status` is not a fallback.

Five Stream store/connection-bridge regressions were traced on 2026-09-26 to
three fixture construction sites that still used raw serde inputs. Preserve their
budget, snapshot, port-selection and hot-reload assertions when completing these
fixtures. Missing-status rejection tests must continue using raw inputs. The
[canonical status guard](../../status/regression-guards/canonical-parent-status-ownership.md)
defines the production contract; do not relax admission to repair test setup.

## Resolution references

- D0095 / `plugin-policy-invalid-generation-retention` — reproduced and fixed by
  scoped publication; current policy is reapplied to every retained executable
  candidate, and the focused current-HEAD regression passed on 2026-09-12.
- `http-plugin-store-request-generation-snapshot` / B0175 — earlier declined
  request-wide proposal, preserved separately as superseded history.
