from compact_receipt import persist
import record as r
body='''
# Require the complete upstream path in PathNormalization forwarding assertions

Six request-client cases in `PathNormalizationForwardViewTestSuite` accept any echoed path beginning with the expected path. `tc_a_default_normalized`, `tc_b_annotation_raw_true` and `tc_c_annotation_norm_false` describe this as accepting a query string attached to the path; the rewrite, multi-step and invalid-annotation cases use the same prefix acceptance. However, the actual selected backend's `catch_all_handler` emits `req.uri().path()` as `Path:` and emits the query separately as `Query:`. Arbitrary path suffixes therefore satisfy a probe intended to verify the complete forwarded path.

A concrete false-PASS witness for `tc_e_multi_step_normalization` is a successful completed backend echo containing `Path: /api/foo/bar/extra` instead of `/api/foo/bar`. Its `p == expected || p.starts_with(expected)` guard returns PASS. Both fixture trees select the `/api` PathPrefix route and the test-http wildcard echo backend, so this different upstream path can still receive a normal 200 response. Similarly, `/api/data-wrong` satisfies the default, normalized-annotation and invalid-annotation assertions; `/rewritten/data-wrong` satisfies the rewrite-precedence assertion. These are source-derived wrong-output witnesses, not observed forwarding failures.

The newer `absolute_form_consumers` case already compares parsed path and query exactly for four host variants. This is useful counterevidence: complete output assertions are feasible without a new server endpoint or production observability. It does not cover the multi-step input or invalid-annotation host, and it does not justify weakening the separate origin-form cases. Production `finalize_request_uri` owns the exact selected path and preserved query; its actual unit tests check complete URI values and the finalization latch. The normalization review contract also identifies path/query preservation as the relevant consumer invariant.

Replace the permissive path-prefix success branches with equality against each case's expected complete path. Use the existing separate-query parser if query preservation is included in that case's stated contract. If an alternate echo format genuinely appends a query to `Path:`, split only at its explicit `?` delimiter before comparing the entire path; arbitrary suffixes must still fail. A small private assertion helper is optional, not a required shared framework. Keep missing-path, transport and body-read failures as failures.

Preserve all case identities, requests, fixtures, expected statuses, raw-versus-normalized and rewrite precedence, optional canonical access-log checks, listener resolution and wrapper deadlines. Do not change normalization, URLRewrite, matching or request/response body ownership in production. Global restart-dependent scenarios and the deliberate mode-agnostic PreserveParam suite remain outside this correction. Verification should challenge each affected assertion with exact success, an appended path segment, an appended non-separator suffix and a missing path, while retaining the real backend echo contract. No execution or measured performance result is claimed.
'''
r.candidate('ARCH-382','path-forwarding-complete-output-assertions','PART-503','/root/analyze_outbound','behavior-defect','P3',body)
body='''
# Require an observed Admin rejection for malformed ConditionSet probes

`condition_set_structural_invalid` in `edgion-tests/src/client/suites/edgion_plugins/plugin_validation_status/plugin_validation_status.rs:286-343` passes its local-mode assertion whenever `TestContext::apply_yaml` returns an error for each of three malformed ConditionSet documents. The error is treated as proof that the Controller rejected both operators or forbidden nesting. That helper erases distinct outcomes into `Result<(), String>`: client construction, PUT/POST transport failure, authentication or authorization denial, retry failure, HTTP 500 and the intended HTTP 400 can all return `Err`. None of those outcomes is inspected by this case.

A concrete conditional witness is a transport outage affecting these three operations after the earlier static status checks completed: each apply returns an error and this case reports that all malformed shapes were rejected without observing any Controller response. Other cases can detect persistent infrastructure failure, but their independent success does not make this operation's failure schema evidence. No current run failure, admission bypass or whole-suite false pass is claimed.

The real decoder deliberately rejects these shapes before resource persistence. `ConditionSet` is a bounded, deny-unknown-fields union, and the actual `condition_set_schema` tests separately distinguish structural decode rejection from semantic rejection of an empty operator. The registered Kubernetes path already performs server dry-run and requires specific decoding/schema-rejection reasons before setting its verification flag. This finding is limited to the local Admin probe; the Kubernetes proof must remain intact.

Use the existing strict admission-observation policy in `status::helpers::expect_admin_admission_rejected`: authenticated precondition and non-persistence checks, an observed HTTP 400, and failed transport/auth/unexpected-status outcomes. Preserve these authored YAML documents and their YAML request boundary; the existing JSON helper needs only a narrow payload seam if reused. Do not route this negative assertion through a generic apply helper that has already discarded the response status. Keep malformed fixture identities isolated and handle unexpected persistence cleanup as failure, without weakening any of the other nine assertions.

Retaining broad errors plus a prior health check is an inadequate alternative because the operation can fail after that check. A display-string match on `apply_yaml` errors would couple evidence to formatting rather than HTTP outcomes. No production admission, ConditionSet semantics, status projection, transport implementation, logging or metrics changes are needed.

Future verification should make this predicate fail on connection refusal, deadline, 401/403, 500, unexpected success and failed non-persistence checks, while the three real malformed YAML documents produce observed 400s. Preserve existing schema unit tests and local/Kubernetes integration execution. This is distinct from ARCH266's Controller projection, ARCH355's structural-versus-reference evidence and ARCH265's cleanup-result ownership. All primary code and decisive real seams were read; review remained static and read-only.
'''
r.candidate('ARCH-385','condition-set-observed-admin-rejection','PART-505','/root/analyze_conversion','behavior-defect','P2',body)
persist('PART-503','/root/analyze_outbound',{
'forward_view.rs':('cf35de30e2754642ca3d9bd12256d1de51c5d262bafa5ed335f571ae0be0f175','Full1-608','ARCH382 six originform prefix assertions admit wrong paths; absoluteform query/path and optional canonical log exact.'),
'mod.rs':('a209adf17af452507e177b765e7f8b893e9223634f3e15c0f60f93c4a0a037e8','Full1-13','Three private suite exports and actual registrations accepted.'),
'path_normalization.rs':('9566e026f18a77ff81c2cd072a4e254b351711228579b7a2e17ebb10b855bde9','Full1-189','Exactroute/pathparam reachability probes match fixtures; narrower prefixprobe doesnotclaim forwarded equality.'),
'preserve_param.rs':('6430b056a9e4e1b98a2f9fa858ba03c3cb89043a2dbc90e7ab47c6dfb60cf8db','Full1-158','Modeagnostic automation/deferred restartcase accepted intentional scope.')},[
('02-gateway/uri-normalization/SKILL.md','ae1252dac6096d74ea5db2d454025b6a4aa41b60e36ec471651614ff6086f440','Full1-217 original normalization architecture.'),
('mandatory-design/normalization-contract.md','8e4d1037e1659df73e005c7552c393435124b5753ec232d8ab2fdf3e578e853d','Full1-161 original invariant.'),
('proxy_http/pg_upstream_peer.rs','1e78b4bf440134f3f8949ab883bc1f767c0e70055164818c35fc0e2251b25eaf','1-145,1717-1775 actual finalizer/tristate forwarding owner.'),
('proxy_http/pg_upstream_peer_test.rs','14ccc89d36290e92ac69aa08b1b152cffb394278085aad72a2693b65dd302399','308-469 actual complete path/query/latch/forwarding matrix units.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','3526-3587 wildcard separatepath/query/headers producer.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','2949-2992 actual registrations.'),
('ForwardView/03_HTTPRoute_default-normalized-forward.yaml','6ba81385080795d432eb3dfeacc21a591fbe2db9c4ff3d91b5ba7ab688c7cb31','Full1-36 local fixture.'),
('ForwardView/03_HTTPRoute_default-normalized-forward.yaml','c0ca1395dd6bc876e452167b893a30a41eb6202ff75206c238f355ae4eb9cd22','Full1-20 K8s fixture.')],
'All18selected fixtureYAML full incl recovered truncation; canonicalnormalizer1-190/realunits1-167,441-490/framework532-600 context. Wrapper30s bounds absoluteform awaits; no unboundedhangclaim. No genericregistry/restartautomation. Personal originalrulesfull/freshunchanged, staticonly/no execution/mutation/supportcredit.',candidate_ids=['ARCH-382'])
persist('PART-505','/root/analyze_conversion',{
'plugin_validation_status.rs':('dba1b4173afbed6e924ae59bbdec7d1b1993da61364cda21b5f2f4d272654776','Full1-406,17562bytes','Ten cases: canonicalControllerFalse/Invalid/message and emptysemanticoperator accepted; ARCH385 local structuralnegative boundary. Pending265local emptycase cleanup result.'),
'mod.rs':('eb2dd0ad096362a4d9dbf81b4d839e0dbf8d16939dde407a513fe867ce099956','Full1-2,97bytes','Actual privateexport accepted.')},[
('client/framework.rs','dd7b89a260befd5283e63c7a405e878918949ad17a618af0b23138312fed0383','379-453 YAML PUT/POST retry/status erasure.'),
('status/helpers.rs','0236de331bc053f9ba00610543ef4e7a8932a9d49e129110f068091d98f823dc','1-74,326-374 controllerprojection/strict admission helper.'),
('verify_plugin_validation_admission.sh','d08a5c3175522fd3ab135fc8d5501e70f3e476feafe7ab7e3ef71c0e552d2fee','Full1-40 K8s specificthree rejection reasons.'),
('PluginValidationStatus/both_operators.yaml','c5c85ebf92cfa52ce6435cd04dc24fab7c2b0925f8c4696fa030212b6a614f3c','Full1-18 actual invalid K8s document.'),
('tests/condition_set_schema.rs','1e1b527a6221836a98e9c7b8b408d9bb5fce4434b0fb39bce0607d6a3b746f20','1-170 actual JSON/YAML structural vs semantic tests.'),
('edgion-controller/src/api/common.rs','2d96d7e138a37e864cf499469fefb2fe14dd1f18fc6661e9bd7850f8a3522dc9','225-243 typedparse400.'),
('handlers/edgion_plugins.rs','9a6ed6dbb4caa2a8a6a5b9bee0e9a9d75344edd9aad3b9eeee5dccc6413dc16a','178-194,426-456,466-490 structural/preparse vs refs status owner.'),
('condition-set-single-owner.md','eb021205b436c1b999f947d3281b2883df3d68ef4f8e1398a2f74544a216ed6f','Full1-48 original boundedcanonical container.')],
'All8local+9K8s valid/advisory fixtures+3invalid full; actualmintflags60-76/61-72/main2889-2909/K8s2598 andtypedConditionSet105-149,850-877/real compileempty536-592 originals inspected. Same265local380-384 deleteErr ignored afterPASS; malformed328 cleanup ignore afteralreadyFAIL no furtherPASSwitness. No266/355 duplicate. Original rules retained/fresh rootonly nested. Staticno execution/mutation/supportcredit.',candidate_ids=['ARCH-385'])
s=r.state();q=s.setdefault('pending_supplements',{}).setdefault('ARCH-265',[])
if 'REPORT-PART-505.json' not in q:q.append('REPORT-PART-505.json')
s['resume_dispatch']['analysis_outbound']=['PART-504'];s['resume_dispatch']['analysis_conversion']=['PART-506','PART-507'];r.save(s)
