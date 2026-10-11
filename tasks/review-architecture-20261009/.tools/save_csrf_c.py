"""Persist exact completed CSRF analysis and candidate; no reviewed-source edits."""
import hashlib
import record as r

body='''# ARCH-349: CSRF cookie-preservation failures publish the issued token

## Claim and current ownership

`test_upstream_cookies_preserved` in `edgion-tests/src/client/suites/edgion_plugins/csrf/csrf.rs:87–137` collects every Set-Cookie value and checks for `upstream_a`, `upstream_b`, and the CSRF cookie. Its failure branch at lines 125–132 formats the entire cookie vector into `TestResult::failed`.

The actual backend emits two fixed upstream cookies. The Gateway appends a freshly generated, signed CSRF token; both local and Kubernetes fixtures select a Secret-backed key and a 7,200-second lifetime. If cookie preservation regresses while CSRF issuance succeeds, the failed assertion contains that complete token. `TestResult::failed` retains the error unchanged, ConsoleReporter prints it, and optional JSON reporting copies it. Synthetic fixture credentials and the test-only topology limit the consequence; this is not evidence of production credentials or an observed disclosure.

## Bounded improvement and alternative

The assertion already computes the useful evidence: each required cookie's presence and the total count. Report those facts and the operation instead of serializing cookie values. Keep full values local to token extraction and comparison where required, and keep every existing success/failure assertion. The other static CSRF diagnostics that explicitly interpolate raw Set-Cookie strings should use the same value-free result policy, without expanding into a repository-wide logging refactor.

A central reporter sanitizer is the inferior alternative: it receives unstructured Strings after token content has already been copied and must guess which fragments are sensitive. Fix the producing diagnostic; no redaction framework, extra runtime allocation, or Gateway change is needed.

## Contracts and future verification

The graded redaction rules and original log-safety page prohibit complete token output. The credential Debug guard explicitly does not protect direct field formatting. The current dynamic CSRF suites already report status and cookie presence without token/key values.

Preserve all three-cookie checks, get_all multiplicity, signed-token validation, safe/unsafe method behavior, missing-key sealing, rotation/deletion/recovery, and the pre-rotation token guard. The accepted append-cookie decision remains valid; this finding concerns its test's failure output, not the production append owner.

Future focused verification should construct the failure with a distinctive issued-token marker and one missing upstream cookie: it must remain failed, identify the missing observation, and omit the marker from console/JSON-bound error data. Retain complete success and missing-CSRF controls and both runners.

## Limits

Static review of actual sources, fixtures and existing test bodies only. No execution or disclosure incident; priority is low because the demonstrated token belongs to the synthetic test topology.
'''
assert hashlib.sha256(body.encode()).hexdigest()=='cf4cdae590ef417e36fea58a30be6df73adebf71e47658b8fd2e3b4c69220612'
r.candidate('ARCH-349','csrf-cookie-failure-token-diagnostic','PART-465','/root/analyze_conversion','behavior-defect','P3',body)
rows=[
 ('edgion-gateway/src/plugins/http/csrf/plugin.rs','e1beb333','Full actual plugin.'),
 ('edgion-gateway/src/plugins/http/csrf/token.rs','8cc8b0a4','Full actual signing/validation.'),
 ('edgion-gateway/tests/unit/plugins/http/csrf/plugin_test.rs','2b4814d2','Full actual plugin unit bodies.'),
 ('edgion-gateway/tests/unit/plugins/http/csrf/token_test.rs','6e27cd06','Full actual token unit bodies.'),
 ('edgion-tests/src/client/dynamic/scenarios/csrf_secret_rotation.rs','45246d05','Full current scenario including fixture-key unit body.'),
 ('edgion-tests/src/client/framework.rs','dd7b89a2','485-531,640-699 failed result retention/report conversion.'),
 ('edgion-tests/src/client/reporter.rs','fe5136f8','1-72,140-167 console and optional JSON error propagation.'),
 ('edgion-tests/src/client/test_client.rs','3f5afa96','3007-3020,3942-3960 actual suite and phase registration.'),
 ('edgion-tests/src/server/test_server.rs','6c38b285','3438-3456 fixed upstream cookie producer.'),
 ('skills/03-coding/01-log-safety.md','74591411','1-120 original token redaction rule.'),
 ('skills/04-review/csrf/regression-guards/csrf-secret-rotation-integration-coverage.md','86ec0957','Full original rotation coverage contract.'),
 ('skills/04-review/csrf/regression-guards/set-cookie-append.md','48e005c9','Full original append ownership.'),
 ('skills/04-review/security/regression-guards/credential-material-debug-redaction.md','f0bbf522','Full original Debug versus direct-field redaction boundary.'),
 ('edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/csrf.rs','ae210363','Full resolver original.'),
 ('edgion-gateway/src/plugins/runtime/plugin_runtime.rs','d80096aa','2318-2340 missing-key seal gate.'),
 ('edgion-tests/integration/conf/EdgionPlugins/Csrf/EdgionPlugins_default_csrf-test.yaml','56c0f1be','Full local Secret-backed key/lifetime fixture.'),
 ('edgion-tests/integration/conf/EdgionPlugins/Csrf/HTTPRoute_default_csrf-test.yaml','9322b37a','Full local selected route.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/Csrf/EdgionPlugins_default_csrf-test.yaml','7be0a9b9','Full Kubernetes key/lifetime fixture.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/Csrf/HTTPRoute_default_csrf-test.yaml','e76493f7','Full Kubernetes selected route.'),
 ('edgion-tests/integration/scripts/integration/run_integration.sh','5a6df789','1895-1901 actual suite dispatch.'),
 ('edgion-tests/k8s/scripts/run_k8s_integration.sh','9a858cd8','2545-2558 actual suite dispatch.'),
]
supports={}
for p,prefix,scope in rows:
    assert r.digest(r.REPO/p).startswith(prefix),p
    supports[p]=scope
dispositions={}
for p in r.state()['partitions']['PART-465']['paths']:
    if p.endswith('/csrf.rs'):
        prefix,desc='0800ae46','Full1-435, eight static cases; ARCH349 failure diagnostic serializes issued cookies. No production cookie/signature defect claimed.'
    elif p.endswith('/dynamic.rs'):
        prefix,desc='3811de99','Full1-227 signed issuance/validation, stale-token rejection, missing-material sealing, value-free messages and Drop token cleanup; no independent issue.'
    else:
        prefix,desc='2e92a5ac','Full1-8 explicit exports and token guard ownership; no independent issue.'
    assert r.digest(r.REPO/p).startswith(prefix),p
    dispositions[p]=desc
r.partition('PART-465','/root/analyze_conversion',dispositions,supports,['ARCH-349'],
 'Complete static analysis, no tests/builds/execution/network/Git mutation. Synthetic issued test token only; no observed disclosure or production token impact. Original authority/graded/routing personally read; original Secure/integer-second expiry decisions retained, no duration/watcher/body/TLS changes. Existing lifecycle/status/cleanup findings deduplicated. Supporting files earn zero primary coverage.')
print(r.state()['candidates']['ARCH-349'])
