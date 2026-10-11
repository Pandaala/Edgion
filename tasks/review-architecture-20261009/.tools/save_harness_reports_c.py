"""Record completed current-source harness reports, not new technical analysis."""
import json
import subprocess
import record as r

paths = subprocess.check_output(['rg', '--files', 'edgion-gateway', 'edgion-resources', 'edgion-tests', 'skills'], cwd=r.REPO, text=True).splitlines()
def supports(rows):
    result = {}
    for suffix, prefix, scope in rows:
        matches = [p for p in paths if p.endswith(suffix) and r.digest(r.REPO/p).startswith(prefix)]
        assert matches, suffix
        for path in matches:
            result[path] = scope
    return result

def save(part, analyst, descriptions, rows, ids, limits):
    state = r.state()
    names = state['partitions'][part]['paths']
    assert set(descriptions) == {p.rsplit('/', 1)[1] for p in names}
    dispositions = {}
    for path in names:
        prefix, disposition = descriptions[path.rsplit('/',1)[1]]
        assert r.digest(r.REPO/path).startswith(prefix), path
        dispositions[path] = disposition
    r.partition(part, analyst, dispositions, supports(rows), ids, limits)

save('PART-457', '/root/analyze_outbound', {
 'access_log_extern.rs': ('1d36001e', 'Full 1-791, ten actual cases. No new independent issue: terminal 404/401/204, route provenance, seven sources, entry-query versus rewrite, timeout absence, route precedence, byte cap and JSON escaping respect canonical ownership.'),
 'mod.rs': ('58e116b7', 'Full 1-5 exports and caller registration; no issue.')}, [
 ('observe/access_log/log_ext.rs', 'c03', 'Full implementation 1-276.'),
 ('observe/access_log/log_ext_test.rs', '4756', 'Full 1-258 isolated masking/overwrite/cap/UTF8/escaping/invalid reference.'),
 ('access_log_extern_policy_tests.rs', '12d0', 'Full actual policy defaults, categories, casing and overrides.'),
 ('pg_logging.rs', '972f', '85-145 terminal collection and JSON store.'),
 ('plugins/http/plugin_store.rs', '8b6b', '325-365 cold invalid-entry retain/drop.'),
 ('pg_request_filter.rs', 'e21', '320-365 matched preflight and short circuit.'),
 ('plugins/http/key_auth/plugin.rs', 'a087', '139-181 missing-key 401.'),
 ('plugins/http/proxy_rewrite/plugin.rs', 'c257', '218-254 injected/original query and upstream URI.'),
 ('server/test_server.rs', '6c38b285', '3500-3550 actual delay and catch-all echo.'),
 ('EdgionPlugins/AccessLogExtern/resources.yaml', '7dcb', 'Full current Kubernetes fixture 1-557.'),
 ('claims-access-log-filter.md', 'd81', 'Full original claims-to-access-log contract.'),
 ('ctx-provenance-unmasking-risk.md', '5c95', 'Full original provenance policy.'),
 ('sensitive-mode-test-subprocess-isolation.md', '65c5', 'Full original isolation policy.')], [],
 'Read-only complete terminal analysis. Supporting reads earn zero primary coverage. Off-mode omissions are covered by isolated production-unit tests; private/local and public Kubernetes policy fixtures intentionally differ. Collision checks route precedence only; query fixture lacks a separate upstream-body stimulus control. No executed results or production defect claimed.')

save('PART-459', '/root/analyze_conversion', {
 'all_endpoint_status.rs': ('0d42384c', 'Full 1-807. ARCH347 path-override assertion does not distinguish configured health path from successful wildcard echo. Other schema/cache/header/rate/recovery/zero-endpoint cases assessed; bounded assertion limitations are not new root causes.'),
 'mod.rs': ('e1157a95', 'Full 1-2 exports and caller registration; no independent issue.')}, [
 ('AllEndpointStatus/EdgionPlugins_default_all-endpoint-status.yaml', '7e973', 'Full local probe configuration and health override.'),
 ('AllEndpointStatus/EdgionPlugins_default_all-endpoint-status.yaml', 'ef90', 'Full Kubernetes probe configuration and health override.'),
 ('AllEndpointStatus/HTTPRoute_default_all-endpoint-status-test.yaml', '59a57', 'Full local and Kubernetes selected routes.'),
 ('plugins/http/all_endpoint_status/plugin.rs', '49fca', '486-620, 685-726, 890-925 actual fan-out/path selection and response assembly.'),
 ('plugins/http/all_endpoint_status/plugin_test.rs', '60a', '1-205 rate gate, denial, no discovery and cap-zero cases; selected read only.'),
 ('server/test_server.rs', '6c38b285', '575-584, 717-762, 3526-3549 health and wildcard actual handlers.'),
 ('scenarios/all_endpoint_status_tls.rs', '44522137', '54-143 current dynamic assertions; equal inbound/override paths do not prove precedence.'),
 ('all-endpoint-status-tls-identity.md', 'fa93', 'Full original endpoint aggregation and TLS identity contract.')], ['ARCH-347'],
 'Read-only full primary and relevant call-chain analysis. No tests/builds/execution. Endpoint-detail/header cases permit empty collections and Connection-close case does not inspect its header; recorded limitations, not separate findings. Existing ARCH064 malformed-method cause is distinct. Supporting evidence earns zero coverage.')

save('PART-460', '/root/analyze_outbound', {
 'bandwidth_limit.rs': ('d8900c62', 'Full 1-285, all three cases. Existing ARCH280 historical aggregate/window assertion cause also occurs here; no duplicate candidate. Functional and BackendRef selection assertions retain value, without isolated throughput evidence.'),
 'mod.rs': ('dc244f47', 'Full 1-5 exports/caller registration; no issue.'),
 'plan.md': ('6e03542a', 'Full 1-46 primary plan. Metrics ownership description is stale: Gateway annotations actually enable test latency. Invalid-rate fail-open applies only to direct constructor bypassing current validation; no standalone architecture issue.')}, [
 ('plugins/http/bandwidth_limit/plugin.rs', 'e5b8', 'Full stateless per-stream duration/body and invalid sentinel implementation.'),
 ('plugins/http/bandwidth_limit/plugin_test.rs', '5ff4', 'Full actual bandwidth unit bodies.'),
 ('pg_upstream_response_body_filter.rs', 'f4cb', '335-380 current dispatch.'),
 ('listener_gateway_info.rs', '66b', '210-245 immutable metrics annotation publication.'),
 ('pg_logging.rs', '972f', '245-295 and 331-369 canonical metrics recording.'),
 ('test_metrics.rs', '8b04', 'Full actual test metrics ownership.'),
 ('metrics_helper.rs', '6a64', '35-85, 150-208, 312-340 identities/counts/fetch/key filters.'),
 ('server/test_server.rs', '6c38b285', '815-846 actual POST echo.'),
 ('bandwidth-limit-invalid-rate-no-tracing.md', '20ab', 'Full original runtime failure/logging decision.'),
 ('scoped-response-body-filter-dispatch.md', '86f', 'Full original scoped stage dispatch contract.')], [],
 'No new ID; pending supplement to existing ARCH280 requires independent extension review before changing its approved claim. Compliance case can create a prior one-second terminal sample. Metrics case ignores HTTP status/body-read outcome and searches lifetime maximum across all backend/latency series; prior success can certify current failure. Proposed same-cause extension: fresh attributed increments and scoped latency with fallible exchanges, existing deadlines, no counter resets/new product metrics/generic measurement framework. Static source reasoning only; transport/server/scheduling contribute measured time. Obsolete plan fixtures remain bounded documentation follow-up.')

save('PART-461', '/root/analyze_outbound', {
 'basic_auth.rs': ('232c9e25', 'Full 1-658 all twelve cases. No independent candidate: exact statuses, real upstream credential-presence probe, grouped credentials and canonical access-log observation preserve owners.'),
 'dynamic.rs': ('c6114a09', 'Full 1-312 four phase suites/nine cases. Initial/update user and group assertions distinguish runtime generation and escaped realm; scenario reset belongs to ARCH264.'),
 'mod.rs': ('79262faa', 'Full 1-7 private modules and explicit exports; no issue.')}, [
 ('plugins/http/basic_auth/plugin.rs', 'b5437d3b', 'Full prepared generation, failure delay/challenge, anonymous branch and credential removal.'),
 ('plugins/http/basic_auth/plugin_test.rs', 'cc9f26ef', '1-110 and 190-286 extraction/error, sensitive identity, generation and exact log shapes.'),
 ('plugins/http/common/auth_common.rs', '0a184e33', '173-204 and 250-265 challenge escaping/order and conditional failure delay.'),
 ('plugins/http/common/auth_common_test.rs', '93937760', '194-247 actual challenge matrix and response-order assertions.'),
 ('scenarios/basic_auth_dynamic.rs', 'ad4a80de', 'Full current normal/grouped orchestration, apply/wait and absent restoration; existing ARCH264.'),
 ('client/dynamic/ctx.rs', 'f37853b6', 'Full 1-238 current status acknowledgement barrier, phase runner, apply and polling.'),
 ('client/dynamic/status.rs', '9ac8c3e9', 'Full current selected-controller status helper; these phase cases do not call it directly.'),
 ('client/test_client.rs', '3f5afa96', '2588-2633 registration/phase selection and port keys.'),
 ('server/test_server.rs', '6c38b285', '953-975 actual auth-header presence probe; full credentials are not emitted.'),
 ('02_EdgionPlugins_default_basic-auth-hide-creds.yaml', '7a97c42c', 'Full local and Kubernetes hide-credentials fixtures.'),
 ('04_EdgionPlugins_default_basic-auth-delay.yaml', '724802ab', 'Full local and Kubernetes failure-delay fixtures.'),
 ('DynamicTest/initial/EdgionPlugins_basic-auth-dynamic.yaml', '92de9079', 'Full local and Kubernetes initial users.'),
 ('DynamicTest/updates/EdgionPlugins_basic-auth-dynamic.yaml', 'c8373b2c', 'Full local and Kubernetes updated users.'),
 ('DynamicTest/initial_grouped/EdgionPlugins_basic-auth-dynamic-grouped.yaml', '73918c31', 'Full local and Kubernetes initial groups.'),
 ('DynamicTest/updates_grouped/EdgionPlugins_basic-auth-dynamic-grouped.yaml', '01587bc0', 'Full local and Kubernetes updated groups.'),
 ('mandatory-design/dup-key-first-wins.md', '1a10abfb', 'Full original duplicate user policy.'),
 ('mandatory-design/group-in-ok-log.md', 'c1d534be', 'Full original auth identity observation.'),
 ('mandatory-design/iron-rules.md', 'cea828cc', 'Full original security rules.'),
 ('mandatory-design/shared-apply-auth-failure-delay.md', '41a45660', 'Full original canonical failure delay.'),
 ('regression-guards/auth-challenge-realm-quoted-strings.md', '010d6f99', 'Full original challenge escaping contract.')], [],
 'Static review only; no execution/builds/tests or other mutations. Success timing upper bound is advisory, stage-log case checks presence rather than outcome, and anonymous-access behavior lacks integration coverage in this fixture set. Status acknowledgement has an independent timeout: nominal phase duration does not bound every attempt. No new architecture candidate; original rules and topics personally read by analyst. Supports earn no coverage.')
state=r.state()
state.setdefault('pending_supplements', {}).setdefault('ARCH-280', [])
if 'REPORT-PART-460.json' not in state['pending_supplements']['ARCH-280']:
    state['pending_supplements']['ARCH-280'].append('REPORT-PART-460.json')
state['resume_dispatch'] = {'analyst_conversion':'PART-462', 'analyst_outbound':'PART-463', 'independent_verifier':'ARCH-340', 'note':'Four completed terminal reports saved; current-C support binding checked.'}
r.save(state)
print('Recorded PART457, PART459, PART460, PART461: ten primary source files.')
