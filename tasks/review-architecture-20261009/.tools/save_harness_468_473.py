"""Save completed read-only terminal reports; primary credit remains hash-bound."""
import subprocess
import record as r

paths=subprocess.check_output(['rg','--files','edgion-gateway','edgion-resources','edgion-controller','edgion-tests','skills'],cwd=r.REPO,text=True).splitlines()
def selected(rows):
    out={}
    for suffix,prefix,scope in rows:
        matches=[p for p in paths if p.endswith(suffix) and r.digest(r.REPO/p).startswith(prefix)]
        assert matches,suffix
        for p in matches:out[p]=scope
    return out
def save(ident,analyst,primary,rows,limits):
    dispositions={}
    part=r.state()['partitions'][ident]
    if part['status']=='reviewed':
        return
    for p in part['paths']:
        prefix,scope=primary[p.rsplit('/',1)[1]]
        assert r.digest(r.REPO/p).startswith(prefix),p
        dispositions[p]=scope
    assert len(primary)==len(dispositions)
    r.partition(ident,analyst,dispositions,selected(rows),[],limits)
    subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)

save('PART-468','/root/analyze_conversion',{
 'direct_endpoint.rs':('acdfd27a','Full1-152. Concrete selected IP:30001 success dye, membership403 and missing-input fallback/no-success-dye; accepted ownership. Header observation does not claim body completion. Uses canonical selected context port.'),
 'mod.rs':('8806835d','Full1-2 exports and actual registration; no issue.')},[
 ('plugins/http/direct_endpoint/plugin.rs','8faeb4c0','Full actual plugin.'),
 ('plugins/http/direct_endpoint/plugin_test.rs','58e35239','Full actual plugin unit bodies.'),
 ('backends/validation.rs','acb04a20','Full canonical backend validation.'),
 ('pg_upstream_peer.rs','1e78b4bf','1498-1555 actual selected peer.'),
 ('run_test_client.sh','8ab474c6','208-223 current Kubernetes endpoint-inventory producer.'),
 ('DirectEndpoint/EdgionPlugins_default_direct-endpoint.yaml','9f7005a6','Full local plugin.'),
 ('DirectEndpoint/EdgionPlugins_default_direct-endpoint.yaml','2ac5d9cd','Full Kubernetes plugin.'),
 ('DirectEndpoint/EndpointSlice_default_direct-endpoint-backend-slice.yaml','952a5d18','Full authoritative local endpoint.'),
 ('DirectEndpoint/Service_default_direct-endpoint-backend.yaml','e010b521','Full Kubernetes selector-backed Service.'),
 ('backendref-upstream-jump-plugin-scope.md','6e8adbd3','Full original explicit route ownership contract.'),
 ('shared-service-backend-ref-predicate.md','17aa6b12','Full original canonical predicate.')],
 'Complete static analysis; real endpoint-inventory producer falsifies fixed-loopback concern. Both fixture trees and actual selected routes/Services read. No ARCH296 extension: framework199-223 borrows ctx.http_port. No tests/builds/execution/network/Git mutation; supporting reads zero coverage. Original rules personally retained and fresh-checked, no nested rules.')

save('PART-469','/root/analyze_conversion',{
 'body.rs':('d264b219','Full1-1079, four bounded ranges and recovered tail. Request-only BodyStorage, one-scan cache, spill/mutation, H1/H2 replay, every-attempt digest, bounded separate http.call/framing/invalid UTF8, physical cap/100Continue assertions preserve owners. Same ARCH296 literal listener-port cause pending extension.'),
 'context.rs':('5f9dcd84','Full1-77. Request-writer/response-reader positive alpha/beta, missing and no-write sequence; fallible64KiB drains and canonical context port. No new cause.')},[
 ('body_ops.rs','3fdd9df6','Full1-236 actual DSL body owner.'),
 ('ctx_value_tests.rs','4d710c9a','Full actual context unit bodies.'),
 ('opaque_body_stream_tests.rs','7ee1da61','Full actual bounded opaque stream bodies.'),
 ('EdgionPlugins/DslBody/resources.yaml','d59f40b2','Full1-556 actual Kubernetes body fixture.'),
 ('DslBody/EdgionPlugins_dsl-req-body-read.yaml','de735d7f','Full local request-body fixture.'),
 ('DslBody/EdgionPlugins_dsl-body-http-call.yaml','de5c53e8','Full local bounded external call fixture.'),
 ('server/test_server.rs','6c38b285','1182-1283 actual retry producer validates digest before counter on every attempt.'),
 ('client/test_client.rs','3f5afa96','1550-1660,3040-3075,3830-3905 selected registration and port resolution.'),
 ('bounded-body-text-reads.md','cad06a91','Full original bounded complete-body rule.'),
 ('http-call-full-body-read-before-truncate.md','9e9234d1','Full original separate bounded exchange contract.'),
 ('request-response-context-continuity.md','2cdf3e2b','Full original continuity contract.'),
 ('integration-port-selection-must-fail-closed.md','c2ff6118','Full original port authority.')],
 'No new candidate; supporting reads zero credit and no execution. Exact expected strings do not pass on empty fallback, so no ARCH348 extension. Pending ARCH296 scope: body.rs URL31180 at29/97/166/258/313/363/742/859/910/1017, raw sockets568/604, H2 sockets806/956 and URL824 ignore selected ctx.http_port. No current failure claimed; retain logical Host/backend30001/framing/timeouts/assertions.')

save('PART-470','/root/analyze_conversion',{
 'dsl.rs':('1b67593c','Full1-2517 consecutive bounded ranges,117908bytes. Explicit scenario/fixture inventory accepted: headers/cookies/context/URI/crypto/control/response/Upgrade/asyncHTTP/Redis/prefix/query/error assertions. No framework extraction/new architecture root. Same ARCH296 URL-port scope extension pending.')},[
 ('plugins/http/dsl/plugin.rs','7948524b','Full1-183 actual compilation/runtime/response async bridge.'),
 ('plugins/http/dsl/lang/vm.rs','830e80cb','1190-1260,1265-1365 actual dispatch/deny/terminate/error policy.'),
 ('mod_test/request_builtins.rs','56b28f97','Full1-370 actual unit bodies.'),
 ('mod_test/response_builtins.rs','bfd60c81','Full1-245 actual unit bodies.'),
 ('mod_test/terminate_tests.rs','c338830a','Full actual termination unit bodies.'),
 ('cached_query_tests.rs','a3bad59f','Full actual cached query bodies.'),
 ('ctx.rs','71647688','3470-3505 canonical frozen query/path views.'),
 ('Dsl/22_EdgionPlugins_dsl-hmac-verify.yaml','65d1a010','Full local and Kubernetes public fixed-vector HMAC fixtures.'),
 ('Dsl/33_EdgionPlugins_dsl-response-http-timeout.yaml','28d97a32','Full local actual timeout fixture.'),
 ('Dsl/33_EdgionPlugins_dsl-response-http-timeout.yaml','c0296c96','Full Kubernetes timeout fixture.'),
 ('terminate-explicit-local-response.md','716cd1b4','Full original explicit terminate contract.'),
 ('response-stage-http-timeout.md','b2755dc1','Full original timeout contract.'),
 ('redis-error-results-bounded-and-redacted.md','0d4e5c2c','Full original bounded/redacted Redis failure contract.')],
 'No new candidate. Four early denials allow expected status despite wrong/missing text or body-read fallback; precise coverage limits, no incident. terminate_with_fallback wording is stale versus current explicit502. Printed HMAC uses entirely public fixed test-secret/path, so no minted credential/ARCH349 extension. All local/Kubernetes numbered01-29 plugins read; shared context support scopes in PART469 remain unchanged. No execution/mutations; supports zero coverage.')

save('PART-471','/root/analyze_outbound',{
 'dynamic.rs':('5d2e2425','Full1-222 positive/negative generation assertions; same ARCH296 listener-port scope extension.'),
 'grpc_error.rs':('7da99bdb','Full1-527 exact native codes/empty termination source+bytecode and upstream counts; no new issue.'),
 'mod.rs':('bd4a6a83','Full1-21 actual exports/case composition checked; no issue.'),
 'response_http_timeout.rs':('b3706ec6','Full1-68 slow success control, short cap markers and complete OK response.'),
 'terminate.rs':('772d29b0','Full1-185 empty502/HEAD/H1close/queued headers/chain/upstream counts; no new issue.'),
 'terminate_dynamic.rs':('c8a70756','Full1-95 three generation outcomes and result-aware scenario reset; no new issue.'),
 'xml.rs':('b9413ac7','Full1-350 H1/H2 replay/retry/mirror/hard limits/physical phase; no new issue.')},[
 ('scenarios/dsl_dynamic.rs','8f23c760','Full actual scenario lacks restore; existing ARCH264 scope.'),
 ('scenarios/dsl_terminate_dynamic.rs','3c0cc150','Full current all-outcome restoration.'),
 ('scenarios/dsl_xml_dynamic.rs','9077ed37','Full current all-outcome restoration.'),
 ('mod_test/terminate_tests.rs','c338830a','Full actual five test bodies.'),
 ('lang/xml_builtins_test.rs','ca08a736','Full1-229 actual XML unit bodies.'),
 ('server/test_server.rs','6c38b285','752-764,1041-1093,1187-1258,3518-3527 actual health/mirror/retry/delay.'),
 ('edgion_gateway_config.rs','bc982f3c','209-235 canonical body schema, no legacy aliases.'),
 ('bounded-xml-primitives.md','6f04ae53','Full original bounded XML contract.'),
 ('terminate-explicit-local-response.md','716cd1b4','Full original, truncated tail recovered.'),
 ('response-stage-http-timeout.md','b2755dc1','Full original timeout contract.'),
 ('DslBody/DynamicTest/updates/to_capped/EdgionGatewayConfig_example-gateway.yaml','e4072a39','Full local current-schema cap update.'),
 ('DslBody/DynamicTest/updates/to_capped/EdgionGatewayConfig_example-gateway.yaml','29c36aca','Full Kubernetes supporting fixture retains legacy body fields; follow-up.')],
 'No new primary-source candidate. Kubernetes capped/restored fixtures still use defaultMaxBodySize/defaultMemoryBufferSize while canonical schema/local fixtures use maxBodySize/storage.memoryLimit; scoped supporting fixture follow-up, no cluster execution claim. Existing ARCH264/296 same roots need independent extension. gRPC driver explicit abort is inconsistent but drop cleanup prevents any proved orphan. Fixed markers limit repeated log attribution; direct wire assertions remain. Supports zero credit; no execution/mutation.')

save('PART-473','/root/analyze_outbound',{
 'denied_named_jump.rs':('89847d5f','Full1-395 including actual fixture validation unit. Real Controller denial/synchronized projections, seven paths, exact policy/body/dye/typeddiagnostic, unique identities and positive receipt/terminal controls; no new issue. Static fixture reuse/local-only agree with original decision.'),
 'dynamic_internal_upstream.rs':('806a2421','Full1-551 eleven rules/direct/weightzero/fallback/dye/repeat/missing/invalid cases use resolved context; no new issue.'),
 'mod.rs':('20a461b3','Full1-4 exports and both registered suites; no child credit.')},[
 ('plugins/http/dynamic_internal_upstream/plugin.rs','95ff4e56','Full1-186 canonical lookup/policy/dye producers.'),
 ('plugins/http/dynamic_internal_upstream/plugin_test.rs','87f669bf','Full1-473 actual selection/denied/weightzero/port unit bodies.'),
 ('routes/http/routes_mgr.rs','03881445','45-149 canonical weighted/named matcher and RefDenied.'),
 ('pg_upstream_peer.rs','1e78b4bf','765-810,990-1033 exact typed failure and genuine-miss fallback.'),
 ('handlers/http_route.rs','826bcdd3','84-112,318-359,675-695 authored denial and ResolvedRefs.'),
 ('handlers/route_backend_refs.rs','52c816a5','1-145 exact authorization and resolve_denials.'),
 ('suites/status/helpers.rs','0236de33','1-244 actual current status helper/native units/authenticated polling.'),
 ('server/test_server.rs','6c38b285','805-875,1041-1120 real echo/synchronous receipt producer.'),
 ('client/access_log_client.rs','f7691b8a','Fresh89-204 typed status/route/backend/attempt/diagnostic getters; retained full unchanged.'),
 ('DynamicInternalUpstream/04_HTTPRoute_default_named-jump-denied.yaml','0ce2bcef','Full actual six local rules and denied/legal siblings.'),
 ('DynamicInternalUpstream/04_HTTPRoute_default_diu-rules-test.yaml','a8e97a23','Full actual Kubernetes identities/weightzero backend.'),
 ('denied-named-jump-fails-closed.md','c6dbb8fa','Full original local-only/static fixture reuse evidence.'),
 ('dye-stage-producer-parity.md','1d157f38','Full original stage/admission/fixture compilation contract.')],
 'Complete static review; fifteen local/seven Kubernetes plus crossnamespace fixtures fully read. Ordinary fallback checks success without selected sibling identity; invalid-name case title is broader than its valid canary stimulus, while direct cases separately cover rejection. Local-only DeniedNamedJump does not prove Kubernetes admission/grant reload or unreachable defensive publisher branches. No builds/tests/network/mutation; supporting reads zero coverage. Original root/graded/routing/topics personally read and current hashes checked.')
state=r.state()
for ident,reports in {'ARCH-296':['REPORT-PART-469.json','REPORT-PART-470.json','REPORT-PART-471.json'],'ARCH-264':['REPORT-PART-471.json']}.items():
    queue=state.setdefault('pending_supplements',{}).setdefault(ident,[])
    queue.extend(p for p in reports if p not in queue)
state['resume_dispatch']={'analyst_conversion':['PART-478','PART-479','PART-480'],'analyst_outbound':['PART-474','PART-475','PART-476'],'independent_verifier':'ARCH-348 plus PART464 supplement','note':'Seed001 revised independent receipt received; report persistence pending. ARCH353/354 complete analyst bodies pending persistence.'}
r.save(state)
