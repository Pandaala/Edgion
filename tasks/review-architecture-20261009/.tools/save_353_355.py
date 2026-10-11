"""Persist completed analyst candidates and their exact primary reports."""
import subprocess
import record as r

def save_candidate(ident,slug,part,body):
    r.candidate(ident,slug,part,'/root/analyze_outbound','behavior-defect','P2',body)

save_candidate('ARCH-353','empty-plugin-log-probe-errors','PART-467','''# Require observed empty plugin logs instead of accepting probe failures

`PluginLogsTestSuite::test_empty_plugin_logs` reports success for every error returned by `send_and_fetch_log`. The helper owns three distinct steps: send the Gateway request, retrieve its terminal record through the AccessLog Store Admin API, and deserialize that record. Its `Result<AccessLog, String>` carries failure from any step, but this case's `Err(_)` arm labels every failure “no access log stored” and passes. A refused data-plane connection therefore passes this case without sending a request; an Admin transport/authentication failure or incompatible log document also passes without establishing empty plugin logs. This is a source-level false-positive witness, not a reproduced integration incident or a claim that the entire suite passes during a permanent outage.

The absence allowance is justified by a comment claiming simple requests may not be stored. The current producer does not implement that policy. `pg_logging::logging` stores the canonical terminal JSON when integration-testing mode is enabled and the request contains `access_log: test_store`, irrespective of whether plugins ran. This helper supplies that header and a unique canonical W3C traceparent. `AccessLogEntry.stage_logs` is omitted when empty, and this suite's existing serde default correctly converts that omission into an empty vector. Thus a successfully retrieved record already represents the legitimate no-plugin outcome.

The Admin endpoint distinguishes a missing store entry through its response envelope; `AccessLogClient` also returns errors for HTTP, transport and JSON failures. Store TTL and bounded admission can make a record unavailable, but those are failures to obtain evidence, not proof that no plugin executed. The preceding store-status case does not repair the boundary: cases can be selected independently, and infrastructure can fail after that check.

Change only the empty-case outcome boundary: retain the successful-record empty-vector assertion and return a failed `TestResult` with the existing helper error on `Err(e)`. Preserve the test identity, request host/path, listener context, trace correlation, retry limits, fixtures and all other assertions. No new production logs, metrics, API, parser framework or client error taxonomy is needed. An alternative allowing explicit not-found would require a documented producer policy and a successful transport witness; current source supplies neither justification for absence nor permission to classify all errors as absence.

Future focused checks should distinguish a valid record with omitted/empty stages, a record with nonempty stages, request failure, Admin failure and malformed record. Preserve terminal-store versus debug-response-header ownership and the accepted development-header limitations. Both fixture trees, actual store/API producer and relevant real unit bodies were inspected. No tests, builds, requests or mutations were performed.
''')

save_candidate('ARCH-354','external-loopback-diagnostic-attribution','PART-472','''# Attribute external-loopback rejection to the current request diagnostic

`skip_matched_localhost_rejected` and `reject_matched_localhost_rejected` treat every received HTTP 500 or 502 as proof that external-loopback admission rejected the selected target. Neither correlates the request with its terminal diagnostic. If selection succeeds and localhost resolution succeeds, but the admission check is allowed or bypassed and the subsequent dial fails, the same 502 satisfies these assertions. DNS or other Gateway failures can also satisfy them. This is a source-level false-positive witness, not an executed security regression or a claim that the complete suite passes after bypassing protection.

The runtime already owns precise evidence. `DynamicExternalUpstream::run_request` publishes an authorized External jump and logs “upstream selected” before address admission. `pg_upstream_peer::external_peer` resolves the domain, calls `validate_resolved_domain_addr`, records `BackendLocalhostNotAllowed` on rejection and retains the rejected address in the canonical upstream attempt. Therefore selection logs alone cannot prove the later security decision. Existing plugin unit tests verify selection, Host mutation order, dye outcome and jump rejection; they do not substitute for this live admission witness.

Both fixture trees select localhost targets. Their policy preconditions differ: the local base omits the loopback override, while the current Kubernetes base explicitly sets `securityProtect.allowLoopbackUpstream: true`. The original upstream-address-safety contract permits that override. A failed localhost dial in the Gateway Pod must not be described as loopback-policy enforcement, and changing the shared Kubernetes allowance globally would disturb unrelated accepted fixture ownership.

For these two rejection probes, retain the expected 500/502 outcome and require the exact current request's existing error diagnostic `BackendLocalhostNotAllowed`. Use a fresh marker, canonical `traceparent_for_marker`, `access_log: test_store`, and the existing authenticated AccessLogClient retry path. Missing, malformed, stale or unavailable evidence fails. Establish the deny-loopback precondition through an isolated suite-owned fixture/publication boundary; preserve the shared Kubernetes allow-loopback base. Until that precondition is established, report the incompatible test configuration rather than passing an ordinary dial error as security evidence. No production policy, error mapping, DNS behavior, metric or trace log changes are needed.

A positive fallback request is useful readiness control but cannot attribute the later failed exchange. Narrowing to 502 alone also cannot distinguish connection failure from rejection. This is the external DNS admission boundary, distinct from ARCH352's upstream certificate-SAN hook. Host-override and dye-label assertions have separate coverage limits and are not folded into this finding.

Future verification should pass genuine loopback rejection with the matching fresh diagnostic, fail unrelated 500/502 or allowed-loopback dial failures, and preserve fallback/reject/regex cases, canonical listener selection and both fixture topologies. All primary implementations, actual producer/client seams, real unit bodies and original decisions were inspected; no execution or mutation occurred.
''')

save_candidate('ARCH-355','managed-overlay-reference-resolution-evidence','PART-474','''# The managed-overlay suite treats structural acceptance as proof that the overlay reference resolved

`ep_resolves_pm_chain` and `pm_validated_accepted` in `edgion-tests/src/client/suites/edgion_plugins/edgion_config_data_managed/edgion_config_data_managed.rs` poll the EdgionPlugins resource and pass when its selected Controller conditions contain `Accepted=True`. Both success messages claim that `overrideRef` resolved to the pre-created RegionRouteOverride. Neither predicate checks `ResolvedRefs`, nor verifies that the plugin actually references the expected ConfigData. The preceding ConfigData checks establish that `data-managed` or `data-validated` exists; they do not establish the plugin's reference edge.

The current Controller deliberately separates these facts. `EdgionPluginsHandler::update_status` constructs Accepted from structural/preparse validation, then separately calls `resolve_override_refs` and constructs ResolvedRefs. A missing, disabled, denied, wrong-type or invalid effective overlay can therefore coexist with an accepted safe base. The actual `test_validate_region_route_override_ref_missing` unit requires missing-overlay preparse to remain clean, parse to continue and reference resolution to report `EdgionConfigDataNotFound`. The original ConfigData architecture likewise requires fail-to-base rather than structural rejection. These are supported states, not malformed diagnostic responses.

A concrete regression witness is changing a fixture plugin's `overrideRef.name` to another syntactically valid, absent name while leaving the pre-created expected ConfigData and valid base unchanged. The existence checks still pass and Accepted can remain True while ResolvedRefs is False. All three managed-overlay cases can then pass despite the claimed managed reference chain being broken. The committed local and Kubernetes fixtures both supply explicit expected references in `edgion-default`; neither topology turns Accepted into a reference-resolution condition. This is distinct from ARCH266: these readers already use the current canonical Controller-local conditions projection.

Within these two existing cases, inspect the returned plugin's actual RegionRoute entry and require the expected override reference identity. In the same bounded poll, require both current-generation `Accepted=True` and `ResolvedRefs=True` from the already-selected Controller observation. Preserve the existing expected ConfigData presence/type checks, authenticated Admin requests, native and partitioned status handling, retry limits and fail-on-read/parse behavior. Missing conditions must keep the predicate unsatisfied. No production status, fallback, admission, logging, fixture ownership or generic test framework changes are needed.

Checking Accepted alone cannot prove the edge, and checking only ConfigData existence cannot detect a mistargeted reference. A traffic-success check alone would also admit the intentionally valid base fallback. Future verification should feed the real predicates healthy, missing-target, mistargeted-reference, stale-generation and absent-condition observations without changing the existing test identities. This review is static; no suite or cluster was executed.
''')

def record(ident,primary,rows,candidates,limits):
    part=r.state()['partitions'][ident]
    dispositions={}
    for p in part['paths']:
        prefix,scope=primary[p.rsplit('/',1)[1]]
        assert r.digest(r.REPO/p).startswith(prefix),p
        dispositions[p]=scope
    supports={}
    for p,prefix,scope in rows:
        assert r.digest(r.REPO/p).startswith(prefix),p
        supports[p]=scope
    r.partition(ident,'/root/analyze_outbound',dispositions,supports,candidates,limits)
    subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)

record('PART-467',{
 'logs.rs':('2c83844e','Full1-427 seven actual cases. ARCH353 empty-case accepts failed observation as absence; successful stage/timing/header assertions preserve terminal-store schema.'),
 'mod.rs':('5afaf292','Full exports and actual registration; no independent issue.')},[
 ('edgion-tests/src/client/access_log_client.rs','f7691b8a','Fresh1-39,284-407 identity/authenticated status/get/retry; full unchanged retained.'),
 ('edgion-gateway/src/routes/http/proxy_http/pg_logging.rs','972f72ec','Fresh1-149 actual integration/header store gate and canonical trace identity.'),
 ('edgion-gateway/src/api/mod.rs','a7b6ba1f','312-388,640-675 actual Admin missing/get/status envelopes.'),
 ('edgion-gateway/src/observe/access_log_store.rs','db0f8065','Full1-245 actual bounded store.'),
 ('edgion-gateway/tests/unit/observe/access_log_store_test.rs','0ecf063c','Full actual missing/expiry/capacity/accounting bodies.'),
 ('edgion-gateway/src/observe/access_log/entry.rs','a0db01fd','145-192 actual empty-stage omission.'),
 ('edgion-gateway/src/plugins/runtime/log.rs','bb8bd222','1-194,485-530 actual plugin/stage serde schema.'),
 ('skills/04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md','901add98','Full original wire versus terminal ownership.'),
 ('skills/04-review/observability/intentional-design/debug-access-log-development-surface.md','f2a9ffa7','Full accepted development-header limits.'),
 ('edgion-tests/integration/conf/EdgionPlugins/DebugAccessLog/HTTPRoute_default_plugin-logs-test.yaml','548929cb','Full local fixture.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/DebugAccessLog/HTTPRoute_default_plugin-logs-test.yaml','548929cb','Full Kubernetes fixture.')],['ARCH-353'],
 'Complete read-only analysis. stage_execution_order verifies names rather than ordering; timing omits nested plugin logs; helper ignores response-body read failure. Fixtures do not enable debug-to-header, so no header-wire coverage claimed. Both full fixture trees, actual producer/client/unit bodies and original rules read; zero supporting coverage. No tests/builds/network/Git mutation.')

record('PART-472',{
 'dynamic_external_upstream.rs':('7066f9b6','Full1-588 twelve cases; ARCH354 loopback500/502 verdict is not attributed to actual admission diagnostic. Other selection/Host/dye limits retained.'),
 'mod.rs':('47a3e11d','Full1-2 exports and caller registration; no issue.')},[
 ('edgion-gateway/src/plugins/http/dynamic_external_upstream/plugin.rs','5e4f071c','Full actual selected External jump producer.'),
 ('edgion-gateway/tests/unit/plugins/http/dynamic_external_upstream/plugin_test.rs','3d40322d','Full1-220 actual units, recovered first100lines.'),
 ('edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs','1e78b4bf','1549-1592,1675-1719 actual resolve/admit/record.'),
 ('edgion-resources/src/diagnostic.rs','06002650','158-177,412-426 exact error diagnostic.'),
 ('edgion-tests/integration/conf/base/EdgionGatewayConfig.yaml','0e024b70','Full local deny-loopback default.'),
 ('edgion-tests/k8s/conf/base/EdgionGatewayConfig.yaml','f93e62d7','Full Kubernetes explicit accepted allow-loopback policy.'),
 ('skills/04-review/ssrf/mandatory-design/upstream-address-safety.md','09bc36a6','Full original loopback override authority.'),
 ('skills/04-review/tls/regression-guards/dynamic-external-upstream-sni-host-validation.md','3ccf019b','Full original Host/SNI safety boundary.'),
 ('edgion-tests/integration/conf/EdgionPlugins/DynamicExternalUpstream/01_EdgionPlugins_default_deu-skip.yaml','5cffc111','Full local selected targets.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/DynamicExternalUpstream/01_EdgionPlugins_default_deu-skip.yaml','4492dac7','Full Kubernetes targets.')],['ARCH-354'],
 'Complete static report; all eight local/seven Kubernetes fixtures and current rules personally read. Host/dye-named cases check upstream-selected log only, while actual unit bodies verify runtime effects; no wire attribution claimed. Current classification correct, no bypass/incident/performance witness executed. No tests/builds/network/Git mutation; supports zero credit.')

record('PART-474',{
 'edgion_config_data_managed.rs':('8e2b8745','Full1-367. ARCH355 two resolution cases use structural acceptance alone; expected ConfigData existence does not prove actual owning reference.'),
 'mod.rs':('e3698d71','Full1-2 actual export/caller registration; no issue.')},[
 ('edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins.rs','9a6ed6db','421-486,530-595,1040-1105 separate Accepted/ResolvedRefs and override resolver.'),
 ('edgion-controller/tests/unit/conf_mgr/sync_runtime/resource_processor/handlers/edgion_plugins_test.rs','f50b5c4d','3055-3103,3175-3224 actual valid/missing-overlay units.'),
 ('edgion-tests/src/client/suites/status/helpers.rs','0236de33','Retained personal full1-244 unchanged canonical selected/native projection and actual units.'),
 ('edgion-tests/integration/conf/EdgionPlugins/EdgionConfigDataManaged/01_EdgionPlugins_data-managed.yaml','6f7c2fca','Full owning reference and valid base.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/EdgionConfigDataManaged/resources.yaml','280beba7','Full five actual fixture documents.'),
 ('skills/01-architecture/05-resources/15-edgion-config-data.md','fa43b542','1-110 original overlay/fail-to-base authority.'),
 ('skills/04-review/resources/intentional-design/edgion-config-data-managed-by-is-inert.md','d65933db','Full1-126 accepted metadata ownership.')],['ARCH-355'],
 'Complete static analysis. All five local fixtures and actual registration2871-2877 read. Selected/native Controller projection already correct, distinct ARCH266. Historical external Center references were only read in local docs, no external verification/traffic coverage. Original authority personally read/current-checked; zero supporting credit. No tests/builds/network/Git mutation.')
print('Saved three analyst candidates and six complete primary source reads.')
