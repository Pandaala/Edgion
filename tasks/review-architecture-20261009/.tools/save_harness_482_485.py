"""Persist four completed reports and only pending scoped status evidence."""
import subprocess
import record as r

paths=subprocess.check_output(['rg','--files','edgion-gateway','edgion-resources','edgion-controller','edgion-tests','skills'],cwd=r.REPO,text=True).splitlines()
def save(ident,analyst,primary,rows,limits):
    d={}
    part=r.state()['partitions'][ident]
    for p in part['paths']:
        prefix,desc=primary[p.rsplit('/',1)[1]]
        assert r.digest(r.REPO/p).startswith(prefix),p
        d[p]=desc
    supports={}
    for suffix,prefix,scope in rows:
        matches=[p for p in paths if p.endswith(suffix) and r.digest(r.REPO/p).startswith(prefix)]
        assert matches,suffix
        for p in matches:supports[p]=scope
    r.partition(ident,analyst,d,supports,[],limits)
    subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)

save('PART-482','/root/analyze_conversion',{
 'global_connection_ip_restriction.rs':('b875641d','Full1-312 two ranges,three authenticated bounded polls. Typed Selector existence/Accepted/activep1/nonemptydescription roundtrip match Controller storage scope; no liveL4 policy coverage claim.'),
 'mod.rs':('e6477d93','Full1-2 export/real runner registration; no issue.')},[
 ('GlobalConnectionIpRestriction/01_EdgionConfigData_gcir-smoke.yaml','980f59ec','Full1-20 local actual Selector fixture.'),
 ('GlobalConnectionIpRestriction/01_EdgionConfigData_gcir-smoke.yaml','7d9534d4','Full1-11 Kubernetes fixture.'),
 ('resources/edgion_config_data/selector.rs','9934e58b','Full1-38 actual consumer-owned profiles.'),
 ('edgion_config_data/selector_test.rs','af954067','Full1-33 actual Selector tests.'),
 ('status_observation.rs','029ec420','1-70 exact partitioned-kind exclusion for ConfigData.'),
 ('handlers/edgion_config_data.rs','e8dfb8e3','150-276 actual native producer.'),
 ('stream/global_connection_ip_restriction/plugin.rs','29dd1fac','Full1-194 actual canonical snapshot/IP consumer.')],
 'Complete static analysis; ConfigData native conditions intentionally remain flat, falsifying ARCH266 lead. Consumer-owned profile membership/fail-to-base retained, no stricter admission. Real TCP runner/unit coverage separate. Both runners and current typed IP/snapshot mechanism/original decisions personally read. No builds/tests/network/mutation; supporting zero credit; original authority/current hashes checked.')

save('PART-483','/root/analyze_conversion',{
 'global_plugins_snapshot.rs':('4a4d14ae','Full1-134 two actual wirestatus/header cases. Response-global and dangling-ref owner accepted; not concurrent reload/onion/large-body proof.'),
 'mod.rs':('07c6a9e5','Full1-3 explicit export/actual registration; no issue.')},[
 ('GlobalPluginsSnapshot/02_EdgionGatewayConfig_example-gateway.yaml','39265d29','Full1-62 local generation source.'),
 ('k8s/conf/base/EdgionGatewayConfig.yaml','f93e62d7','Full1-54 authoritative Kubernetes base includes real global and ghost refs.'),
 ('GlobalPluginsSnapshot/resources.yaml','b3270285','Full1-42 Kubernetes suite fixture.'),
 ('proxy_http/global_plugins_resolve.rs','9c2819a2','Full1-117 pure/silent captured resolver.'),
 ('proxy_http/global_plugins_resolve_test.rs','521f8aad','Full1-231 actual order/defaultns/dangling/position/invalid collision units.'),
 ('edgion-gateway/src/ctx.rs','71647688','2552-2609 request-pinned global snapshot.'),
 ('pg_upstream_response_filter.rs','bfa2ac69','433-472 response callback snapshot and outer global phase.'),
 ('global-plugins-resolve-no-tracing.md','b0830d7a','Full1-68 original mandatory no request-path tracing.')],
 'Complete read-only review. Missing suite-local Kubernetes EGC intentional: shared base owns authoritative global policy. Stale comments15-17/84-85 and local fixture claim per-request warning contradict current silent implementation/original mandatory restraint; documentation-only follow-up, no new log/metric. Other local fixtures/request/runtime/response plugin/real registrations and original no-per-route override read. No tests/builds/network/mutation; supports zero coverage.')

save('PART-484','/root/analyze_outbound',{
 'mod.rs':('b76b83b2','Full1-432 selected shape/root permissions/enforce-measure/metadata/exactJSON/cost/canonical logs; dynamic traffic precedes correct selected Controller status. No new issue.')},[
 ('graphql-guard-selected-shape.md','459b92ef','Full original selected-shape contract.'),
 ('plugins/http/graphql_guard/mod.rs','3fd98f09','Full1-81 actual adapter.'),
 ('plugins/http/graphql_guard/mod_test.rs','e38c507a','Full1-172 actual unit bodies.'),
 ('scenarios/graphql_guard.rs','da587e22','Full1-169 phase/update owner.'),
 ('common/graphql_request.rs','ddfaf01a','Full1-194 shared body/admission/error/cost implementation.'),
 ('GraphqlGuard/10_EdgionPlugins_graphql-guard-measure.yaml','4800f667','Full actual local/Kubernetes measure fixtures.'),
 ('client/test_client.rs','3f5afa96','3080-3110 actual registrations.')],
 'Static/no execution. Both complete fixture trees/dynamic updates/query-cost original personally read. Current Kubernetes tree exists, so historical ARCH304 absence witness must not be reused. Original rules/index/graded/testing fresh unchanged, no nested AGENTS. No backend GraphQL execution/throughput/parity claim; supports zero primary credit.')

save('PART-485','/root/analyze_outbound',{
 'mod.rs':('5e00db3c','Full1-400 exact operation/default/backend/requestbytes/selectedAIconflict/grant/declared-backend invalidation wire checks; no new cause. Same ARCH266 missing owned attachment projection at370-394 pending extension.')},[
 ('graphql-operation-router-declared-backends.md','6290109d','Full original declared-backend contract.'),
 ('canonical-parent-status-ownership.md','4c6bba39','Full original foreign preservation/current-vs-complete contract.'),
 ('plugins/http/graphql_operation_router/plugin.rs','05af968c','Full actual selector/grant/strict jump owner.'),
 ('plugins/http/graphql_operation_router/plugin_test.rs','55f8ccc0','Full1-222 actual unit bodies.'),
 ('scenarios/graphql_operation_router.rs','c8fdf7b9','Full1-92 update/grant phases.'),
 ('GraphqlOperationRouter/DynamicTest/initial/HTTPRoute_graphql-router-dynamic.yaml','5e87997b','Full local parent/backend/sibling identity.'),
 ('GraphqlOperationRouter/DynamicTest/initial/HTTPRoute_graphql-router-dynamic.yaml','4ad02518','Full Kubernetes canonical attachment/crossnamespace identity.'),
 ('server/test_server.rs','6c38b285','796-842,3510-3555 actual original POST/GET bytes and URI fallback.')],
 'Complete static review; both entire fixture trees/current registry/shared request helper/original rules read. Same ARCH266 supplement: requiring Accepted/PartiallyInvalid across every diagnostic parent allows preserved foreign false entries to fail a valid current attachment; select edgion.io/gateway-controller plus edgion-test/edgion-plugins-gateway while preserving current traffic/sibling checks. Existing ARCH264 restoration/ARCH304 tree drift are separate pending roots. No automatically approved extension, backend GraphQL/performance claim or execution/mutation; supports zero credit.')
state=r.state()
queue=state.setdefault('pending_supplements',{}).setdefault('ARCH-266',[])
if 'REPORT-PART-485.json' not in queue:queue.append('REPORT-PART-485.json')
r.save(state)
