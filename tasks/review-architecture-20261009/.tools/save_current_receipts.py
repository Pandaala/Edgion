"""Persist completed independent current-source decisions; coordinator only."""
import json
import shutil
import record as r

def scoped(items):
    result = {}
    for path, prefix, scope in items:
        assert r.digest(r.REPO / path).startswith(prefix), path
        result[path] = scope
    return result

def preserve(ident):
    state = r.state()
    entry = state['candidates'][ident]
    if entry.get('verification'):
        entry.setdefault('verification_history', []).append(entry['verification'])
    old = r.RUN / ('VERIFICATION-' + ident + '.json')
    saved = r.RUN / ('VERIFICATION-' + ident + '-PRE-20261011-C.json')
    if old.exists() and not saved.exists():
        shutil.copyfile(old, saved)
    r.save(state)

seed = scoped([
 ('edgion-gateway/src/plugins/runtime/traits/session.rs', '7b999e', 'Imports and automock declaration; WAF/AI/canonical/semantic operations 548-731 and guard 952-988.'),
 ('edgion-gateway/src/plugins/runtime/session_adapter.rs', 'bb060285', '1590-1865 native/cache/admission/owner/plan/canonical/processor/guardrail ownership.'),
 ('edgion-gateway/src/ctx.rs', '71647688', 'WAF context 140-290; frozen generations 2840-2930; selection/admission phases 3080-3205.'),
 ('edgion-gateway/src/ai/context.rs', 'fd269402', 'Full lazy AI state, context access and installation.'),
 ('edgion-gateway/src/plugins/http/waf/mod.rs', 'd5bca791', '300-370 failure/summary/restore/claim; 435-485 phase one/preflight/format.'),
 ('edgion-gateway/src/plugins/runtime/plugin_runtime.rs', 'd80096aa', '360-440 cached WAF companions; 440-530 AI/conversion compatibility.'),
 ('edgion-gateway/src/plugins/http/plugin_generation.rs', 'aad01465', 'Full source: demand/cache/compilation/release.'),
 ('edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs', '9c2819', '73-112 election scope, priority and tie.'),
 ('edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs', 'e21e8d49', '350-405 pinned winner, physical body cap and body consumers.'),
 ('edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs', 'f4cb74', '130-255 freeze, canonical handoff, AI semantic/ordinary sequencing and terminal cleanup.'),
 ('edgion-gateway/tests/unit/plugins/http/waf/mod_test.rs', '9b75f3', '30-120 mocks; 315-370 response take/install; 1180-1250 format admission.'),
 ('edgion-gateway/tests/unit/ai/protocols/openai/canonical_sse_test.rs', 'ea2c7', '70-120 raw cap and semantic publication.'),
 ('AGENTS.md', 'cba9f532', 'Full original project authority read by verifier.'),
 ('skills/SKILL.md', 'f0dfcd1b', 'Full original knowledge navigation.'),
 ('skills/00-design-rules.md', 'c78c8667', 'Full graded hard rules.'),
 ('skills/04-review/knowledge-routing.md', 'b4e5658c', 'Full original knowledge routing.'),
 ('skills/01-architecture/02-gateway/05-plugin-system.md', 'b650', '1-220 original ownership architecture.'),
 ('skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md', '2d4', 'Full original WAF dispatch/body ownership contract.'),
 ('skills/04-review/plugin-runtime/regression-guards/borrowed-plugin-header-access.md', 'b47', 'Full original borrowed-access performance contract.'),
 ('skills/04-review/plugin-runtime/regression-guards/ai-guard-ordering-publication.md', '8a69', 'Full original guard ordering/publication contract.'),
 ('skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md', 'cc7', 'Full original exhaustive dispatch decision.'),
])
preserve('ARCH-001')
r.confirm('ARCH-001', '/root/verify_ip_union',
 'a5030ff03066531d9bd41f9588f7aa7af3d6423670967bebaf99678407de6079',
 'b975d8002afa2b8222b6f6895a4ba0d4f1cf7a0e79360da203f37397ac936afa',
 ['Current publication pins lazy feature state and generation sequencing, while the automocked common trait still exposes individual WAF/AI operations.',
  'Bounded capability grouping remains actionable architecture debt; borrowed signatures, object safety, mock compatibility, lifecycle and request-path performance require future design verification.',
  'No runtime defect, approved concrete signature, benchmark, executed test or primary coverage is claimed.'], seed)
state = r.state()
state['candidates']['ARCH-001'].update(revalidation_pending=False, revalidated_baseline='BASELINE-AUDIT-20261011-C.json')
r.save(state)

ident = 'ARCH-345'
preserve(ident)
state = r.state()
entry = state['candidates'][ident]
path = r.RUN / entry['path']
assert r.digest(path) == 'b22f8984d9656c5323c34816f7daa35293da5cdfe0880fe2fbcd4a81a1477909'
supports = scoped([
 ('edgion-tests/k8s/conf/EdgionPlugins/BodyPlugins/resources.yaml', '7d3e487e', '112-146 actual request/response entries; 643-675 selected route/backend.'),
 ('edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs', 'b0c92950', '1620-1758 actual Wasm response hook/cases.'),
 ('edgion-tests/k8s/scripts/run_k8s_integration.sh', '9a858cd8', '1-122; 544-2553; 2870-2895 runner selection and lifecycle.'),
 ('skills/04-review/wasm/regression-guards/wasm-body-counter-delta-rerun-safe.md', 'd96', 'Full original counter-delta contract.'),
 ('skills/04-review/testing/regression-guards/body-suite-sweep-registration.md', 'bce', 'Full original body sweep contract.'),
])
receipt = dict(candidate=ident, complete=True, verifier='/root/verify_ip_union',
 decision='not-actionable', disposition='resolved-current-source',
 input_sha256=r.digest(path), approved_claim_sha256='912fadef497df69c4b534c2e56168422f2c070149567854b11c5d6abb63fe1dc',
 primary_coverage=0, evidence=['Current Kubernetes response entry explicitly selects pluginConfig.string=response-body-probe. Request-stage selection is unchanged.',
 'The independently checked guest mount, selected route, nonempty 200 response and positive counter delta remain in place.',
 'The historical configuration gap is resolved in current source; no executed Kubernetes pass or artifact/source equivalence is claimed.'],
 support_selected={p:dict(sha256=r.digest(r.REPO/p), scope=scope) for p,scope in supports.items()},
 limits='Static independent review only; no writes outside review artifacts, execution, builds, tests, network or Git mutation.')
filename = 'VERIFICATION-ARCH-345-RESOLUTION-20261011-C.json'
(r.RUN/filename).write_text(json.dumps(receipt, indent=2)+'\n')
path.write_text(path.read_text().replace('status: candidate', 'status: resolved', 1))
entry.update(status='resolved', sha256=r.digest(path), verification=receipt | {'evidence':filename}, revalidation_pending=False, revalidated_baseline='BASELINE-AUDIT-20261011-C.json')
r.save(state)

supports = scoped([
 ('edgion-tests/src/client/suites/edgion_plugins/all_endpoint_status/all_endpoint_status.rs', '0d42384c', '688-740 path override; 630-680 zero-endpoint predicate; final registration.'),
 ('edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs', '49fca3de', '170-235 schema; 490-626 path and endpoint results; 690-730 assembly; 830-933 aggregation.'),
 ('edgion-tests/src/server/test_server.rs', '6c38b285', '575-597 health; 712-727 wildcard; 753-770 OK; full catch-all 3526-3548.'),
 ('edgion-tests/integration/conf/EdgionPlugins/AllEndpointStatus/EdgionPlugins_default_all-endpoint-status.yaml', '7e973e74', 'Full local health override and bounded probe.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/AllEndpointStatus/EdgionPlugins_default_all-endpoint-status.yaml', 'ef90bdec', 'Full Kubernetes health override and bounded probe.'),
 ('edgion-tests/integration/conf/EdgionPlugins/AllEndpointStatus/HTTPRoute_default_all-endpoint-status-test.yaml', '59a5736b', 'Full selected host, ExtensionRef and Service port.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/AllEndpointStatus/HTTPRoute_default_all-endpoint-status-test.yaml', '59a5736b', 'Full Kubernetes route and selected backend.'),
 ('edgion-gateway/tests/unit/plugins/http/all_endpoint_status/plugin_test.rs', '60a518c8', '350-430 deadline and ordinal assembly, duplicate and failed results.'),
 ('edgion-tests/src/client/dynamic/scenarios/all_endpoint_status_tls.rs', '44522137', 'Inbound echo; 85-128 exact endpoint assertions. Equal override/inbound paths do not distinguish precedence.'),
 ('skills/04-review/backends/regression-guards/all-endpoint-status-tls-identity.md', 'fa93e183', 'Full original aggregation and TLS identity contract.'),
 ('skills/05-testing/SKILL.md', '4ad0fb8f', 'Full original navigation and meaningful coverage requirements.'),
 ('skills/05-testing/02-k8s-integration-testing.md', '50f34117', '1-75 fixture ownership and address-independent assertions.'),
])
r.confirm('ARCH-347', '/root/verify_ip_union', '25257d6868cca0f78fb1f6838a9d3a05aa6132dd4486946d01a0f45bae73a141',
 '3df2128f054e2d669fd18701d3b18784617cf24030ef4f5f7489c4140a8ca7fb',
 ['Wildcard /anything returns 200 echo; outer 200, summary and positive success count cannot prove the configured health override.',
 'Existing endpoint result/status/error/body fields support nonempty results, status 200, null error and exact OK body without new product observability.'], supports)
print('Persisted ARCH-001, resolved ARCH-345, confirmed ARCH-347; zero primary coverage.')
