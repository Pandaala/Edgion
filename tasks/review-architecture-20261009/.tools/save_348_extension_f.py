"""Exact verifier-authored eleven-read expansion; no coordinator substantive rewrite."""
import record as r
import hashlib,json,subprocess
s=r.state();p=r.RUN/s['candidates']['ARCH-348']['path'];assert r.digest(p)=='2e29c73ecbde3613ec404979261967904ddf6685a24058d188e4ed4b3cb99a31'
body=p.read_text().split('---',2)[2]
old='Two registered Canary anti-spoof cases and CredentialInjector\'s shared fail-closed assertion convert response-body read failures into successful evidence of absence.'
new='Two registered Canary anti-spoof cases, CredentialInjector\'s shared fail-closed assertion, six ExtProc cases, ForwardAuth\'s strict-status case and HmacAuth\'s hide-credentials case convert response-body read failures into successful evidence of absence or successful path discrimination.'
assert body.count(old)==1;body=body.replace(old,new)
anchor='\n## Bounded improvement and alternatives\n'
addition='''
The pending REPORT-PART-476 ExtProc supplement exposes the same result loss in six registered fast-tier cases. `processor_closes_before_request_verdict` and `processor_closes_before_response_verdict` default reads at lines 1849 and 1890; `fail_close_returns_500`, `bad_verdict_fail_closes` and `streamed_degenerate_continue_and_replace_fail_closes` do so at lines 3040, 3116 and 3330. All five require HTTP 500 and use `is_sanitized_fail_close_body`, whose accepted generic shapes include an empty body. A failed read therefore passes its generic-body and prohibited-detail checks. Actual `ExtProc::fail_mode` returns `ErrResponse { status: 500, body: None }` on fail-close, so successfully completed empty 500 responses must remain accepted. Existing fail-mode unit bodies protect the fail-open/fail-close outcome distinction; they do not observe an integration response body's completion.

ExtProc's sixth case, `fail_open_passes` at lines 3052–3087, requires HTTP 200 for GET `/get`, defaults the body read at line 3071, then passes whenever `x-ext-proc-req` is absent. The local and Kubernetes dead-target fail-open fixtures route to the existing port-30001 backend. Its catch-all handler serves `/get` with server, method, path and header lines. An unreadable, empty or unrelated 200 cannot establish that this backend received the bypassed request. The separate `streamed_fail_open_passes_original_body` case requires the positive `untouched-payload` marker and does not pass on a substituted empty string; it is not an additional false-PASS site in this extension.

The pending REPORT-PART-480 ForwardAuth supplement is `success_excludes_200_treats_as_failure` at lines 1445–1512. Both topology fixtures set `success.statusCodes: [204]` while the request uses `Bearer valid-token`. The real auth producer returns HTTP 200 JSON with `status: "ok"` and `user: "test-user"`; the ordinary backend `/health` returns plain `OK`. Production classification intentionally rejects the auth 200 under this configured status list and relays that response. The case defaults a failed body read at line 1469, then its alternate `status == 200 && !backend_body` branch accepts the empty substitution, or any unrelated completed 200 that lacks its backend markers. A status of 200 alone cannot distinguish these two paths. This is an observation defect, not a production success-classification defect or a reason to reject legitimate empty 204 auth responses in the separate custom-status case.

The pending REPORT-PART-491 HmacAuth supplement is `hide_credentials_removes_auth_header` at lines 177–225. It sends a valid signed GET `/headers`, requires HTTP 200, defaults the body read and passes when the lowercased string lacks `authorization`. Both topology fixtures enable `hideCredentials`; actual HmacAuth success removes the configured credential headers before forwarding. The real `/headers` producer returns a JSON object with an object-valued `headers` field. A read error or unrelated empty 200 provides no evidence about those upstream headers. This does not establish that current production stripping fails or that any credential was disclosed.
'''
assert body.count(anchor)==1;body=body.replace(anchor,addition+anchor)
body=body.replace('Keep observation ownership in the two Canary cases and the existing CredentialInjector helper.','Keep observation ownership in the existing cases and the CredentialInjector helper.')
anchor='\nKeeping defaulting is shorter but conflates failed observation with verified absence.'
addition='''
For the five ExtProc sanitized-500 cases, keep HTTP 500 and the complete existing generic-body/prohibited-detail predicate, but apply it only after a successful complete read. Accept `Ok("")`; require no echo shape or nonempty error message. For `fail_open_passes`, keep HTTP 200 and the existing processor-header absence assertion, but first require a successfully completed fixture echo identifying port 30001, GET `/get` and its header section. Preserve the fake-processor modes, dead target, configured failure policy, request/response phase distinction, streamed degenerate case and fast/full tier placement.

For ForwardAuth's strict-status case, remove the loose alternate PASS branch. Require a successful complete read and parsed authored auth JSON with `status == "ok"` and `user == "test-user"`, in addition to the existing HTTP 200 and backend-exclusion assertion. Unreadable, empty, malformed or unrelated bodies fail. Preserve the valid token input, `/health` request, strict `[204]` configuration and the separate custom-204 case; do not change denial relay, configured client headers or production status/body classification.

For HmacAuth, require a successful complete read and the actual `/headers` JSON shape with an object-valued `headers` member before applying the existing authorization-absence assertion. Missing or invalid shapes fail rather than becoming an empty header map. Preserve the existing negative marker check, valid signing helper, method/path/Host/date/signature inputs, HTTP 200, hide-credentials configuration and all other authentication cases. Do not emit the echoed headers, signature, body or credential material in new observation-error diagnostics.
'''
assert body.count(anchor)==1;body=body.replace(anchor,addition+anchor)
body=body.replace('A general response/assertion framework adds no necessary value for these three read sites.','A general response/assertion framework adds no necessary value for these case-local read sites.')
anchor='\n## Independent disposition and limits\n'
addition='''
Future focused verification for the accepted extensions must require read failures after their expected statuses to fail. Each ExtProc sanitized-500 predicate must still accept a successfully completed empty body and reject its existing prohibited details. The fail-open predicate must pass only on the intended complete `/get` echo without the processor header. ForwardAuth must pass the authored auth JSON and fail plain backend `OK`, unrelated JSON and empty/unreadable 200 bodies. HmacAuth must fail unreadable, empty, malformed or missing-headers observations; valid fixture-shaped observations must retain the existing authorization-presence rejection. Keep all current identities, requests, fixtures, assertions and deadlines, without a new production metric, log or response-body owner.
'''
assert body.count(anchor)==1;body=body.replace(anchor,addition+anchor)
old='Independent static verification confirms the original Canary cause with this revision and explicitly accepts the pending REPORT-PART-464 CredentialInjector supplement into ARCH-348.'
new='Independent static verification preserves the approved Canary and REPORT-PART-464 CredentialInjector claims, and explicitly accepts the same-root REPORT-PART-476 ExtProc, REPORT-PART-480 ForwardAuth and REPORT-PART-491 HmacAuth extensions into ARCH-348.'
assert body.count(old)==1;body=body.replace(old,new)
body=body.replace('Current source still contains all three defaulting reads.','Current source still contains the eleven scoped defaulting reads: two Canary, one shared CredentialInjector read, six ExtProc, one ForwardAuth and one HmacAuth.')
body=body.replace('Make each body read fallible','Make each scoped body read fallible')
assert hashlib.sha256(body.encode()).hexdigest()=='4e7f439b7eadbd9c02236b29e0b9ec37f3647af3aeceaf2d7f15532af8729d09',hashlib.sha256(body.encode()).hexdigest()
rows=[
('edgion-tests/src/client/suites/edgion_plugins/canary/canary.rs','1f57a04d862d06c7bc5f7823eebeb6fb74c9f4a90fbd14e55cf8f19170405ec8','48-62,220-260,565-607 existing extractor/cases; unchanged priorcontext retained.'),
('edgion-tests/src/client/suites/edgion_plugins/credential_injector/credential_injector.rs','a4beac17b5ed69f8419c7db0255538e3714cf882869fd485777e7a781f54288d','293-329 helper/staticdynamiccallers/deletedselection; unchanged originalcontext retained.'),
('edgion-tests/src/client/suites/edgion_plugins/ext_proc/ext_proc.rs','13cfcc27863ff2c2e160a768df1089a8fd4231357a1bc4b92572bdd238699824','1-42,1586-1595,1827-1907,3024-3125,3304-3388,3575-3690 predicate/sixcases/excludedpositive/registration/realunit.'),
('edgion-tests/src/client/suites/edgion_plugins/forward_auth/forward_auth.rs','bae663b7e9c58fba85ad094cc5c321557ce4a2cbac139ea2e72bf824eaa61774','1-78,1380-1515,1618-1653 strictstatus/loosealternate/custom204/registry.'),
('edgion-tests/src/client/suites/edgion_plugins/hmac_auth/hmac_auth.rs','200ab40f3d91f9dc6bef4d52257e53579d58a6ad58e6ab811f99d6e3458c1edf','1-66,177-228,345-377 inputs/actualcase/registry.'),
('edgion-tests/src/server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','540-752,815-833,881-930,3526-3550,4370-4406,4524-4618 actualrouter/echo/health/headers/catchall/authJSON.'),
('edgion-gateway/src/plugins/http/ext_proc/plugin.rs','f6bf3eb695ab83fbd0173ac72c8a14055ca8c90a1a50fc15b77fe20b5a0cdd7f','130-178 actual failpolicy/goodnext/empty500/canonicaloutcome.'),
('skills/04-review/security/mandatory-design/fail-closed-error-body-no-internal-detail.md','2f4d74bd5bfb3335de18481b56331b7692a53524138ae221f1c9a6e0266ccdc1','Full original empty/genericbody permission/internaldetail prohibition/observation distinction.')]
supports={}
for p,sha,scope in rows:assert r.digest(r.REPO/p)==sha,p;supports[p]=scope
oldreceipt=r.RUN/'VERIFICATION-ARCH-348.json'
(r.RUN/'VERIFICATION-ARCH-348-PRE-EXTENSION-20261011-F.json').write_bytes(oldreceipt.read_bytes())
r.confirm('ARCH-348','/root/verify_ip_union','2e29c73ecbde3613ec404979261967904ddf6685a24058d188e4ed4b3cb99a31','5af48e1b2d06c14c8b97ed8904386b4a01fa6ad8951dcf8cb7b4d35f5c9faa51',[
'Independent exact merged eleven-read claim preserves approved Canary/CredentialInjector and accepts476ExtProc/480ForwardAuth/491HMAC.',
'Completed empty500 stays legal; failopen needs actualfixture; custom204 remains separate; positive streamedpayload case excluded.',
'Original rules/topic decisions/currentbothfixtures/producers/realunits personally inspected, zero primary or execution.'],supports,replacement_body=body)
s=r.state();q=s.get('pending_supplements',{}).get('ARCH-348',[])
s['pending_supplements']['ARCH-348']=[f for f in q if f not in ['REPORT-PART-476.json','REPORT-PART-480.json','REPORT-PART-491.json']]
r.save(s)
subprocess.run(['python3',str(r.RUN/'.tools/checkpoint.py')],check=True)
print('KeyAuth498 remains pending independent extension; not included automatically.')
