"""Persist the completed Canary report and exact analyst substantive candidate."""
import hashlib
import record as r

body = '''# ARCH-348: Canary anti-spoof tests pass after an unreadable echo response

## Claim and current ownership

Two registered Canary integration cases convert response-body read failures into successful evidence of header removal: `spoofed_dye_header_stripped_on_fallback` in `edgion-tests/src/client/suites/edgion_plugins/canary/canary.rs:220–257`, particularly line 241, and `spoofed_dye_header_stripped_when_kill_switched` at lines 565–604, particularly line 586.

Each sends a forged `X-Canary-Active` header to the fixture's `/echo` route, requires HTTP 200, then calls `response.text().await.unwrap_or_default()`. A body-read error becomes an empty string. The absence check consequently returns PASS without observing the backend's headers. The shared client has a five-second request timeout, while the case wrapper allows thirty seconds; a body-read timeout can therefore reach this PASS branch before the outer timeout. This is a conditional false-PASS path in the tests, not evidence that Gateway stripping is currently broken or that the authored echo fixture normally fails.

The real echo handler emits server, method, path, and header lines. Both local and Kubernetes fixtures bind these cases to the stable backend and declare the request dye name, including the internally disabled Canary case. Both runners execute the static Canary suite.

## Bounded improvement and alternative

Keep outcome ownership inside these two cases. Fail on an unsuccessful body read using a value-free operation diagnostic. Before checking header absence, require the completed body to identify the expected fixture echo, using its existing server-port extraction and path/header markers. Preserve the forged-header assertion itself. Two explicit fallible reads are sufficient; a private case-local predicate is optional if it clearly reduces duplication. No transport, registry, or production abstraction is needed.

The current defaulting is shorter but cannot distinguish failed observation from verified absence. Pure stripping unit tests are also insufficient substitutes for these end-to-end wiring assertions.

## Preserved contracts and verification

Preserve expected HTTP 200, hosts, path, fallback behavior, and the distinct internal kill-switch case. The runtime installs and strips the dye plan before plugin gating; original dye-stage knowledge explicitly requires that ordering. Existing unit bodies cover once-per-name stripping, case-insensitive names, missing plans, and response-only rules. Change neither that owner nor dye generation/rendering, body storage, response streaming, access logging, or credentials.

Future verification should require a body-read failure after HTTP 200 to fail, a completed echo containing the forged header to fail, and the expected completed echo without it to pass, while retaining local and Kubernetes execution.

## Limits

Static source, fixture, registration, and existing test-body review only; no integration run or observed incident. This is distinct from scenario restoration, cleanup propagation, status projection, and metric-observation findings.
'''
assert hashlib.sha256(body.encode()).hexdigest() == 'b20b6a29b7f15951a2a377490bc5914904677d23e35aacb4edf094f9b383e8a1'
if 'ARCH-348' not in r.state()['candidates']:
    r.candidate('ARCH-348', 'canary-anti-spoof-unreadable-echo', 'PART-462', '/root/analyze_conversion', 'behavior-defect', 'P2', body)
rows = [
 ('edgion-tests/src/client/framework.rs', 'dd7b89a2', '95-125, 552-589 client and case timeouts.'),
 ('edgion-tests/src/server/test_server.rs', '6c38b285', '815-858 complete echo and header renderer.'),
 ('edgion-tests/src/client/test_client.rs', '3f5afa96', '2758-2780 registration.'),
 ('edgion-tests/integration/scripts/integration/run_integration.sh', '5a6df789', '1928-1936 selected local suite.'),
 ('edgion-tests/k8s/scripts/run_k8s_integration.sh', '9a858cd8', '2545-2556 current suite dispatch.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/Canary/resources.yaml', '07081fb9', 'Full 1-621 actual current fixtures.'),
 ('edgion-tests/integration/conf/EdgionPlugins/Canary/06_EdgionPlugins_default_canary-invalid-fallback.yaml', 'facbde65', 'Full fallback plugin.'),
 ('edgion-tests/integration/conf/EdgionPlugins/Canary/20_EdgionPlugins_default_canary-kill-switch-strip.yaml', 'c2f95926', 'Full internally disabled plugin.'),
 ('edgion-tests/integration/conf/EdgionPlugins/Canary/07_HTTPRoute_default_canary-invalid-fallback-test.yaml', '011118d9', 'Full actual fallback route.'),
 ('edgion-tests/integration/conf/EdgionPlugins/Canary/21_HTTPRoute_default_canary-kill-switch-strip-test.yaml', 'f3cfdb02', 'Full actual disabled route.'),
 ('edgion-gateway/src/plugins/http/canary/plugin.rs', 'a4cd7f40', 'Full 1-184 plugin.'),
 ('edgion-gateway/src/plugins/runtime/plugin_runtime.rs', 'd80096aa', '2949-2986 runtime ordering.'),
 ('edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs', '41254c9e', '261-289 extension wiring.'),
 ('edgion-gateway/src/plugins/runtime/session_adapter.rs', '2f9adeec44dac2b08f4135373744df00673344602abc1f742420ebd289053833', '390-423 borrowed dye operations; coordinator fresh 370-435 read and complete current-C diff establish WAF extraction leaves dye implementation unchanged.'),
 ('edgion-gateway/tests/unit/plugins/runtime/session_adapter_test/dye_strip_tests.rs', '864b1620', 'Full actual stripping bodies.'),
 ('edgion-gateway/tests/unit/plugins/http/canary/plugin_test.rs', 'c9af5443', '366-480 four actual bodies.'),
 ('skills/01-architecture/02-gateway/entry-level-dye.md', 'e9d6906e', 'Full original dye owner.'),
 ('skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md', '1d157f38', 'Full original dye stage contract.'),
 ('edgion-tests/src/client/dynamic/scenarios/canary_hot_swap.rs', 'c1a7efbd', 'Full actual scenario; same ARCH264 restoration cause.'),
 ('edgion-tests/src/client/dynamic/status.rs', '9ac8c3e9', 'Full 1-19; Canary does not consume this helper.'),
]
supports={}
for path,prefix,scope in rows:
    assert r.digest(r.REPO/path).startswith(prefix), path
    supports[path]=scope
state=r.state()
dispositions={}
for path in state['partitions']['PART-462']['paths']:
    if path.endswith('/canary.rs'):
        prefix,desc='1f57a04d','Full 1-666. ARCH348 two anti-spoof absence predicates pass after body-read failure; remaining routing/dye/fallback/rejection/ordering assertions assessed.'
    elif path.endswith('/dynamic.rs'):
        prefix,desc='592b3192','Full 1-141. Initial/final backend and dye assertions exercise both generations; restoration belongs to existing ARCH264.'
    else:
        prefix,desc='db335177','Full 1-5 explicit exports and caller registration; no independent issue.'
    assert r.digest(r.REPO/path).startswith(prefix), path
    dispositions[path]=desc
r.partition('PART-462','/root/analyze_conversion',dispositions,supports,['ARCH-348'],
 'Complete static source and actual fixture/registration/unit-body analysis; supports zero primary coverage. No tests/builds/execution/network/Git mutation. Conditional false-PASS test outcome only, no current production strip defect or incident. No ARCH266 extension: scenario does not consume status helper; ARCH264 is existing scenario restoration owner.')
print(r.state()['candidates']['ARCH-348'])
