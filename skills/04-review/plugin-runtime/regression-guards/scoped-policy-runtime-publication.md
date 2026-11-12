---
name: scoped-policy-runtime-publication
description: Review GatewayClass policy publication, retained DSL/WAF candidates, shared entry ownership, captured HTTP/stream admission scopes, and fail-closed handling of missing or malformed Gateway-level/EGC-global plugin references.
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
source, attachment and binding inputs. Before early RequestId execution, HTTP
requests capture one immutable listener publication containing the HTTP and GRPC
tables, full Gateway index and per-Gateway scopes. Shared listeners capture it
before owner selection. Global, Gateway, route and delayed backend references
use the selected scope from that same publication.
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

## Route and request publication lifetime

HTTP and GRPC writers capture typed manager Weak identities with their exact
compiled table Arcs and commit these pairs with source admission facts together. Their rebuild mutex remains held through the publisher commit;
DashMap entry and route-unit cache guards are released first. The only nested
order is family rebuild mutex -> publisher mutex. Publisher composition never
loads managers or other stores, and no writer holds both family rebuild mutexes.
Gateway-driven rebuilds follow the same order. Complete Gateway entries use the
canonical listener projection, including metrics, SNI, DNS and attachment proof;
same-key metadata changes participate in cold equality and index reuse.

A request retains one publication Arc in its existing plugin-state pointer slot,
without request-time map construction or locks. Unique listeners expose their
scope directly. A shared listener prebuilds one level of selected-owner views;
matching chooses one child with the same table/index Arcs. Children retain no
parent or sibling map. Cold publisher construction initializes the shared empty
view. Route-family deltas copy the complete listener-to-table Arc map on the
cold path (O(listener count)); this accepted simplicity tradeoff adds no route
or table recompilation and no separate incremental publication protocol. Captured absence is a shared empty view, distinct from
not-yet-captured state; later hooks cannot acquire a newly created listener.
Ordinary no-route behavior remains 404 after earlier metadata validation; unique
unavailable owners retain early 503, shared owners retain matched-owner 503, and
native gRPC keeps its existing UNIMPLEMENTED/fallback priority. Strict Weak route
Arc/index gates remain unchanged. Retired views are released with their requests.

The physical HTTP proxy retains its original HTTP and GRPC manager Arcs solely
as construction identities. Each family accessor compares that stable identity
with its captured Weak token and returns either the captured table or the cold
empty table for that family. It never reloads a manager table. gRPC matching and
`has_routes()` use the same gated table, so an inaccessible recreated gRPC manager
cannot suppress surviving HTTP fallback. Ordinary updates keep manager identity;
old captured requests may finish. Retirement of one family does not hide the
other. This preserves existing manager reachability, not arbitrary EGC/UID
rebinding safety; unsupported Gateway recreation still requires restart.

This closes a route-table/admission publication race exposed by a RateLimitRedis
reload test; independent resource kinds still arrive eventually. Namespace
Selector eligibility continues to rely on Controller accepted parents. Ordinary
HTTP/GRPC does not gain Gateway UID revocation. TLS handshake publication remains
separate; the request's SNI/Host checks use its captured full Gateway index.

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

Route plugin demand, AI admission and adaptive facts project cold keys from
canonical loans of actual effective HTTP/GRPC fields. Invalid owner, discriminator,
target/name or effective namespace cannot borrow a same-name plugin. Valid
sibling descriptors retain their demand; valid missing keys retain repair behavior.
Optional cold rule keys preserve ordinals for captured-scope resolution, CORS/WAF
selection and log declarations. Selected backend lookups use the same captured
scope and retain both hits and misses through stages and retries.

An invalid rule ExtensionRef retains its original four-stage position and fixed
error attribution. Preflight dispatches its existing request-filter Arc through
the canonical runner before ordinary/CORS filters; existing leading policy and
configuration gates keep precedence. Backend restrictions remain selected-only.
No reference creates a second policy snapshot, Grant authority or hot-path key
formatting. Lifecycle verification below must distinguish in-process publication
changes from live source/admission testing.

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

Semantic default/name EdgionPlugins targets may select one bare or qualified
raw source. A collision publishes an empty-spec sealed denial, independent of
Bypass, without borrowing either candidate's declarations. Selections retain
the raw owner; collision clears retention and recovery uses only the current
survivor. See the [LKG contract](../intentional-design/edgionplugins-key-scoped-last-known-good.md).
Adaptive delta notifications use semantic target keys; inventories remain raw.

### Actual HTTP attachment field binding (C1)

HTTP attachment cold consumers now share actual EGC/Gateway field validation. Invalid slots are replaced by the empty sentinel before canonical PluginTarget classification (`http_attachment_keys`; see gw-core-02b below), so invalid owners cannot borrow a same-name live plugin. Valid sibling order and existing captured scope ownership remain. A Missing target contributes no class-scope compiled entry; how a miss is enforced depends on the attachment point (see the gw-core-02 section below). Valid default/bare ambiguity remains a distinct sealed denial, independent of Bypass and without LKG resurrection.

### Missing Gateway-level and EGC-global HTTP targets fail closed (gw-core-02)

Trigger: an EGC `globalPluginsRef` entry or a Gateway `edgion.io/edgion-plugins`
key names an `EdgionPlugins` that is absent from the published class scope
(deleted, renamed, mistyped, pruned by GitOps before the referrer, moved between
namespaces). Before the fix, `PluginScope::resolve_http_runtimes`
(`edgion-gateway/src/plugins/publication/views.rs`) used `filter_map` and dropped
the key, so `resolved_globals` / `GatewayScope::resolved_http` silently lost the
plugin list and requests were proxied without gateway-wide auth/ACL/WAF/rate
limits. Route ExtensionRefs (500) and stream plugins (Deny) already failed closed.

Correct behavior: on an available scope, `resolve_http_runtimes` keeps every key
in order and substitutes one shared `PluginRuntime::missing_reference()` per
publication for each miss. That runtime is sealed through the canonical
`seal_fail_closed` / `RejectFilter` path (`MissingEdgionPlugins` in the plugin
log), so the early preflight gate and the request stage both answer 500 before
upstream. No `tracing` is added (cold publication path; see
[no-tracing rule](../../observability/mandatory-design/global-plugins-resolve-no-tracing.md)).
The placeholder is not inserted into `PluginScope::http`, so ExtensionRef
resolution, `http_log_fields` and AI/GraphQL rule admission keep their own miss
semantics. An unavailable scope still prepares nothing because requests answer
503 first. A request keeps the slice it captured; a later publication in which
the target arrives rebuilds the scope and new requests run the real plugin.

No per-reference opt-out (`optional: true`) was added. This is a decision
resting on the frozen failure-policy rule in `skills/00-design-rules.md` (no new
fail-open/fail-close config shape): an optional reference would act as one.
Revisit only with an explicit product requirement for "apply if present".

Limits: a missing global target denies every request of the class, and on a
single-Gateway listener the early preflight gate denies before route matching,
including ACME HTTP-01 challenges on that listener (the same reach as the
ambiguous/invalid-config gates). On shared listeners the scope is captured only
after route matching, so unmatched requests still get 404. Malformed reference
slots are denied the same way (gw-core-02b below).

Verification: unit tests
`prepared_global_slice_matches_reference_order_and_tracks_publication`,
`prepared_gateway_slice_matches_keys_and_reuses_unchanged_owner`,
`prepared_gateway_slice_keeps_ambiguous_placeholder`
(`edgion-gateway/tests/unit/plugins/publication/mod_test.rs`) and
`early_global_generation_and_missing_reference_stay_pinned_and_deny`,
`deferred_gateway_owner_pins_hits_and_misses_when_first_known`
(`edgion-gateway/tests/unit/routes/http/proxy_http/pg_early_request_filter_test.rs`).
Integration: `global_plugins_snapshot_missing_gateway_ref_fails_closed`
(EdgionPlugins/GlobalPluginsSnapshot, private Gateway on port 32007, local mode
only): asserts the 500, zero upstream attempts and `MissingEdgionPlugins` in the
access log. The full local integration run passed on 2026-11-07 (286 suites,
0 failed). Only the Gateway-annotation path has an integration case; the
EGC-global path is covered by unit tests only, and the K8s fixtures have no
equivalent case. A Gateway without listeners also gets a slice (placeholders for
keys outside the class signature); it serves no traffic, and the slice stays a
pure function of (scope, keys), so reuse remains correct when listeners appear. The shared `example-gateway` EGC (local overlay,
`edgion-tests/k8s/conf/base/EdgionGatewayConfig.yaml`, and the BodyDynamic/DslBody
K8s dynamic-update copies) must never carry a dangling or malformed global
reference, because it would deny every suite on that class; the former intentional `ghost` reference
was removed from all of them.

Re-evaluate if attachment references gain an explicit optional/required contract,
or if Controller admission starts withholding Gateways with unresolved plugin refs.
Related: ctrl-proc2-02 (fixed) reports missing global targets as the consuming
GatewayClass `ResolvedRefs=False`; see [shared config status](../../status/regression-guards/shared-config-status-deployment-independence.md).

### Malformed Gateway-level and EGC-global references fail closed (gw-core-02b)

Trigger: a Gateway `edgion.io/edgion-plugins` token or an EGC `globalPluginsRef`
entry that is not a valid reference at all (uppercase or `_` in the name, invalid
namespace, extra `/`, empty segment), or an attachment owner whose own identity is
invalid (every slot of its list is then an `Err`). The invalid-owner case cannot
arise in Kubernetes mode (EGC is cluster-scoped; Gateway names are API-validated);
whether file or etcd mode can deliver such an owner is unverified, so that case
has unit evidence only. Neither path is schema-validated
against object-name syntax: the annotation is free-form and the EGC CRD gives
`globalPluginsRef[].name` only `minLength: 1`, so Kubernetes accepts such typos.

Root cause: `HttpPluginAttachments::entries()` returns one
`Result<PluginAttachmentKey, ReferenceError>` per slot, but the Gateway publication
consumers (`GatewayInput::from_gateway` and the EGC globals in
`Inputs::scope_plan`, `publication/publisher.rs`) projected them with `.flatten()`. An `Err` slot
vanished before resolution, received no gw-core-02 placeholder, and requests were
proxied without the intended plugin list. The Controller only reported it
(Gateway `ResolvedRefs=False/InvalidPluginsRef`; EGC `Accepted=False` and the
consuming GatewayClass `Accepted=False/InvalidParameters`) and still published
the resource.

Correct behavior: the single projection `http_attachment_keys`
(`edgion-gateway/src/runtime/store/listener_gateway_info.rs`, beside the canonical
`gateway_listener_entries` projection) maps every `Err` slot to the
empty string, keeping order. `scope_plan` removes `""` from compile demand, and
`classify("")` cannot succeed (`MissingNamespace`), so the sentinel is never a
`PluginScope::http` key and can never borrow a live plugin. `resolve_http_runtimes`
therefore substitutes the shared gw-core-02 `missing_reference()` runtime: 500
before upstream, `MissingEdgionPlugins` in the plugin log, the same early-gate
reach and limits as a missing target. `GatewayInfo.plugin_store_keys` uses the
same projection so its parity with publication keys is preserved;
`http_log_fields("")` stays empty.

Decision on the open admission question: the Controller keeps reporting status
and does not withhold or reject the Gateway/EGC. Authority: the 🔴 lenient
admission rule (`skills/00-design-rules.md` section N) makes status-plus-downstream
enforcement the default; hard rejection would remove the operator's signal. Denying in the data plane per
Gateway (annotation) or per class (globals) matches the gw-core-02 missing-target
contract, while admission rejection would be a separate cross-layer change. The
plugin-log token deliberately reuses `MissingEdgionPlugins`; the status message
names the malformed field. Controller `plugin_ref_mgr` registration still skips
invalid slots, which is correct because no source can ever match them.

Verification (2026-11-12): unit tests
`prepared_slices_keep_malformed_slots_in_order_and_recover_when_corrected`
(including a raw source stored under the empty key, which is never borrowed),
`actual_global_fields_preserve_siblings_and_keep_invalid_slots_fail_closed`,
`actual_gateway_owner_cannot_borrow_same_name_plugin_from_live_inventory`
(`edgion-gateway/tests/unit/plugins/publication/mod_test.rs`),
`actual_gateway_attachment_projection_keeps_invalid_slots_and_owner_as_sentinels`
(`inputs_test.rs`) and the listener-info parity tests
(`tests/unit/runtime/store/listener_gateway_info_test.rs`); the full
`edgion-gateway` lib suite passed (6843 passed, 0 failed, 19 ignored).
Integration: `global_plugins_snapshot_malformed_gateway_ref_fails_closed`
(EdgionPlugins/GlobalPluginsSnapshot, private Gateway on port 32008, local mode
only) asserts the 500, zero upstream attempts and `MissingEdgionPlugins`.
The targeted local suite run passed on 2026-11-12 (three cases, including the
existing gw-core-02 missing-target case; cache-residue and requeue checks
passed). That run skipped two unrelated environment pre-flights (macOS loopback
aliases unused by this suite; an existing broken agent-doc link elsewhere); it
is not a full integration run.
As with gw-core-02, the EGC-global path has unit coverage only and the K8s
fixtures have no equivalent case.

Re-evaluate if the Controller starts rejecting or withholding resources with
malformed plugin references, if attachment fields gain schema-level name
validation, or if a distinct invalid-reference diagnostic token is required.

## Actual stream attachment owner boundary (C2)

Stream publication inputs bind actual Gateway/TCPRoute/TLSRoute/UDPRoute owners. Empty configured-invalid sentinels never enter stream demand/ID/source selection; connection publication returns Unavailable before lookup. Preserve valid stage selection, shared-listener equivalence, Bypass behavior and old captured views. Route units retain their earlier backend owner gate. See the [canonical stream attachment contract](../intentional-design/stream-plugin-store-key-normalization.md).
