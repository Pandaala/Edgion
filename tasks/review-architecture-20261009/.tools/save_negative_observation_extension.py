"""Persist the exact independently approved merged negative-observation claim."""
import hashlib
import record as r

body='''
# ARCH-348: Negative response-body assertions must fail on unreadable observations

## Claim and current ownership

Two registered Canary anti-spoof cases and CredentialInjector's shared fail-closed assertion convert response-body read failures into successful evidence of absence. The common cause is loss of the observation result: `response.text().await.unwrap_or_default()` substitutes an empty string, and the following negative predicate accepts it. HTTP status checks remain valuable but do not prove that the body was successfully observed.

`spoofed_dye_header_stripped_on_fallback` in `edgion-tests/src/client/suites/edgion_plugins/canary/canary.rs:220–257` and `spoofed_dye_header_stripped_when_kill_switched` at `:565–604` send a forged `X-Canary-Active` header to `/echo`, require HTTP 200, then default a failed body read at lines 241 and 586. An empty substituted body contains no `x-canary-active`, so both cases can return PASS without observing the backend's headers. The shared client has a five-second request timeout and these cases retain the thirty-second wrapper deadline; an earlier body-read error can reach the PASS branch before the wrapper expires. This is a conditional false-PASS path, not evidence that Gateway stripping is currently broken or that the authored echo normally fails.

The real echo handler emits server, method, path and header lines. Relevant local and Kubernetes fixtures bind both cases to the stable backend on port 30001 and declare the request dye name, including the internally kill-switched Canary entry. Current client and runner registrations include the static Canary suite. The expected completed echo is therefore available as positive observation evidence before testing header absence.

`assert_fail_closed` in `edgion-tests/src/client/suites/edgion_plugins/credential_injector/credential_injector.rs:293–329` requires HTTP 500, then defaults a failed body read at line 304. Its lowercase absence check rejects the four authored markers `client-spoof`, `bearertoken`, `credentialinjector` and `credential_injector`; an empty substituted string passes all four checks. The static `missing_secret_fails_closed` case and the dynamic `credential_injector_dynamic_deleted` case both call this helper. The dynamic scenario's deleted-phase factory actually selects that suite, and both topology fixture sets provide the referenced Secret/plugin/route and deletion target. These are two live uses of the same helper, not two separate cleanup or generation findings.

A successful complete empty error body is legitimate. The original fail-closed body contract explicitly permits `body: None`, and actual CredentialInjector rejection code and existing unit bodies retain empty-body 500 outcomes. The defect is accepting a failed read as if it were that successfully completed empty body. It does not establish credential disclosure, failure of the current production empty-body path, or a whole-suite false pass. The existing marker list is preserved; this finding does not claim it proves every possible secret or internal detail is absent.

## Bounded improvement and alternatives

Keep observation ownership in the two Canary cases and the existing CredentialInjector helper. Make each body read fallible and return a failed TestResult with a value-free operation diagnostic on error. Do not print body contents, credential values or raw dependency errors merely to explain failed observation. No shared-client, transport, runtime registry or production abstraction is needed.

For Canary, require the successfully completed body to identify the existing fixture echo before checking header absence: use the existing server-port extraction with expected port 30001 and the actual GET `/echo` and header-section markers. Preserve the forged-header assertion. A completed empty or unrelated HTTP 200 body is not evidence that the intended backend saw the stripped request. Two explicit reads and checks suffice; a small case-local predicate is optional if it reduces actual duplication without becoming an echo framework.

For CredentialInjector, retain expected HTTP 500 and inspect only a successfully read complete body for the existing four prohibited markers. Accept `Ok("")`; require neither an echo shape nor a nonempty error message. Preserve the static missing-Secret and dynamic deleted-Secret identities, requests, spoofed input, lifecycle/status barriers, fixture selection and existing deadlines. The nearby `assert_upstream_header` and Connection-nomination request helper already propagate body-read errors and supply a bounded local precedent; do not rewrite their separate positive-header, capture, diagnostic or cleanup contracts.

Keeping defaulting is shorter but conflates failed observation with verified absence. Pure stripping and credential unit tests cannot replace the end-to-end response observations. A general response/assertion framework adds no necessary value for these three read sites.

## Preserved contracts and future verification

The runtime installs and strips the dye plan before plugin invocation, conditions and Canary's internal kill switch. Original dye-stage knowledge and existing unit bodies preserve once-per-name stripping, case-insensitive names, missing plans and response-only rules. Entry disable and Canary's internal kill switch remain distinct. Do not change that owner, selection/fallback policy, dye generation/rendering, deferred credential injection, mirrors, canonical access logs, body storage or response streaming.

Preserve both Canary hosts and HTTP 200 expectations, stable-backend routing, local and Kubernetes registrations, the CredentialInjector 500 contract and dynamic phase order, and all live test identities/assertions. Retain canonical binary-owned logging, redaction and observability restraint, existing metrics, BoringSSL and product/workqueue boundaries. These are test-observation corrections and need no production semantic change.

Future focused verification should require a body-read error after Canary HTTP 200 or CredentialInjector HTTP 500 to fail. For Canary, the expected complete echo containing the forged header must fail, the expected complete echo without it must pass, and empty/unrelated bodies must fail. For CredentialInjector, a successfully completed empty body and a completed generic body without the existing prohibited markers must pass, while each prohibited marker must still fail. Exercise both static missing-Secret and dynamic deleted-Secret callers without weakening their current lifecycle checks, and retain local/Kubernetes execution coverage.

## Independent disposition and limits

Independent static verification confirms the original Canary cause with this revision and explicitly accepts the pending REPORT-PART-464 CredentialInjector supplement into ARCH-348. Original rules, mandatory fail-closed integration gates, dye-stage producer/strip ownership, permissible fail-closed body shapes, actual cases/callers, relevant fixtures, registrations and unit bodies were inspected. Current source still contains all three defaulting reads. Historical successful runs in knowledge pages are not fresh verification of these conditional branches.

ARCH337 concerns metric readers that erase scrape failures; ARCH280 concerns missing measured traffic windows; ARCH342 concerns positive BackendTLS header-marker evidence. Their corrections do not own these negative response-body predicates. Scenario restoration, cleanup propagation and status projection are also separate causes. This review ran no tests, builds, services or network probes, changed no source/artifact/Git state, and observed no incident or secret leak. Supporting reads earn zero new primary coverage.
'''
assert hashlib.sha256(body.encode()).hexdigest()=='edc70e8db1be3531c868fa610f599614ac4078d6f4768e3d498291420b2bfcf8'
rows=[
 ('edgion-tests/src/client/suites/edgion_plugins/canary/canary.rs','1f57a04d','Full current suite; anti-spoof bodies, server-port parser and positive contrasts.'),
 ('edgion-tests/src/client/suites/edgion_plugins/credential_injector/credential_injector.rs','a4beac17','1-350,420-637 full helper/static+dynamic callers and parser test; not full file.'),
 ('edgion-tests/src/client/dynamic/scenarios/credential_injector_dynamic.rs','e27aa327','1-65 actual deleted-phase factory/order.'),
 ('edgion-tests/src/client/framework.rs','dd7b89a2','95-125,535-580 client five-second timeout/case thirty-second wrapper.'),
 ('edgion-tests/src/server/test_server.rs','6c38b285','815-839,881-920 actual echo markers and separate headers endpoint.'),
 ('edgion-gateway/src/plugins/http/credential_injector/plugin.rs','773f8ab1','Construction/reject/run_request actual empty500/deferred owner.'),
 ('edgion-gateway/tests/unit/plugins/http/credential_injector/plugin_test.rs','6ddb9975','Full actual missing/invalid material/deadline/deferred/empty500 unit bodies.'),
 ('edgion-gateway/tests/unit/plugins/runtime/session_adapter_test/dye_strip_tests.rs','864b1620','Full actual four strip test bodies.'),
 ('edgion-gateway/src/plugins/runtime/session_adapter.rs','54470cf1','2190-2213 current dye install and once-per-name strip.'),
 ('edgion-gateway/src/plugins/runtime/plugin_runtime.rs','d80096aa','2928-2989 strip before filter invocation.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/Canary/resources.yaml','07081fb9','1-40,88-173,573-621 actual Service/fallback/internal-switch documents.'),
 ('edgion-tests/k8s/conf/EdgionPlugins/CredentialInjector/resources.yaml','ab921608','96-113,223-250 actual missing Secret plugin and route.'),
 ('skills/04-review/plugin-runtime/regression-guards/dye-stage-producer-parity.md','1d157f38','Full original ordering and internal-switch distinction.'),
 ('skills/04-review/security/mandatory-design/fail-closed-error-body-no-internal-detail.md','2f4d74bd','Full original authority permits body None.'),
 ('skills/04-review/testing/regression-guards/mandatory-integration-gates-fail-closed.md','fe6af9c2','Full original failed-observation contract.'),
]
supports={}
for p,prefix,scope in rows:
    assert r.digest(r.REPO/p).startswith(prefix),p
    supports[p]=scope
r.confirm('ARCH-348','/root/verify_ip_union','d5680860a5dd5347e5ecff9c3f77771c1f78cc291890bc4a9c2210e3a0dc0e09',
 '7ac4779cbfba3541833ea314f32b2d80432f7c0b5550e63c73e1608cfc6b874e',
 ['All three defaulting reads remain current; same negative-observation cause independently confirmed.',
  'CredentialInjector must preserve legitimate completed empty error body; Canary additionally needs actual fixture echo evidence.',
  'No production stripping defect, secret disclosure, executed incident or whole-suite false-pass claim.'],supports,replacement_body=body)
state=r.state();state.get('pending_supplements',{}).pop('ARCH-348',None)
state['candidates']['ARCH-348'].update(revalidation_pending=False,revalidated_baseline=state['current_baseline']);r.save(state)
print('Exact merged ARCH348 approved claim saved; no new issue or primary coverage.')
