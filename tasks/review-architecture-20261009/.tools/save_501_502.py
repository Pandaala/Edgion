from compact_receipt import persist
import record as r
body='''
# Construct an encoded, invalid signature for the OIDC signature-rejection probe

`test_jwks_invalid_signature_returns_401` in `edgion-tests/src/client/suites/edgion_plugins/openid_connect/openid_connect.rs:277` generates an RS256 JWT and changes its last encoded character to `a` (or `b`). The test then accepts any 401 as proof that a tampered signature was rejected. This construction does not preserve the token's encoding contract, so the test does not isolate the signature-verification boundary named by its identity.

The in-repository signing key and matching test-server JWK are a 2048-bit RSA pair. Its 256-byte signature has a 342-character unpadded base64url encoding. The last sextet therefore contains two data bits and four unused zero bits: canonical final characters are `A`, `Q`, `g`, or `w`. Both replacement characters have nonzero unused bits. The probe consequently creates a noncanonical encoding rather than guaranteeing a well-encoded, changed signature. A strict decoder can reject this before cryptographic verification; a decoder that ignores unused bits can instead retain the same final signature byte when its original high bits match the replacement. No fresh dependency execution or upstream decoder behavior is claimed here.

This distinction is material to the repository's authentication contract. `token_verify.rs` performs the real JWKS decode, and `error.rs:301–315` separately classifies base64/JSON/token-format errors and `InvalidSignature`; both project to 401. Auto mode permits fallback for the former and excludes the latter. The existing response-byte-cap review guard already documents correcting another OIDC fixture so it reaches the intended format-fallback branch rather than a different failure. The local and Kubernetes JWKS fixtures select `JwksOnly`, so this case's status-only assertion cannot distinguish the two boundaries. The adjacent valid-JWT case verifies the matching key and subject but does not repair this negative witness.

Keep the existing case identity and its 401 assertion, but construct the negative token by splitting the generated compact JWT, decoding the signature, flipping one signature byte, and re-encoding with the existing unpadded base64url codec. Preserve the original header and claims. Fail the probe if fixture decoding or reconstruction fails; optionally verify that the changed segment remains decodable and differs in decoded bytes. Mutating an early encoded sextet with an explicit changed-data-bit contract is a smaller alternative, although decoded-byte mutation makes the invariant clearer.

This is a bounded test-fixture correction, not a production authentication change. Preserve discovery, key selection, scope checks, error redaction, Auto fallback policy, existing fixtures, test identities, listener selection and transport behavior. No new logging, production visibility, generic JWT mutation framework, measured performance claim, or observed authentication bypass is proposed.
'''
r.candidate('ARCH-381','oidc-invalid-signature-fixture-encoding','PART-502','/root/analyze_outbound','architecture-debt','P3',body)
persist('PART-501','/root/analyze_conversion',{
'matched_route_meta.rs':('8a5cb58b85776d0945e0f334dc161939a4222393cc6a54d5b2ef12064247a4c9','Full1-107','Canonical CtxSet/matchedroute selector/explicit AccessLogExtern/W3C terminal log; actual metadata five values/dottedslash key. Accepted, same296literal31180 onlypending.'),
'mod.rs':('87c13e234cd11782d40cce19f4e7cdaa36969fa6609d72a4acd39eb8ee2ebb12','Full1-5','Minimal actual export accepted.')},[
('MatchedRouteMeta/01_EdgionPlugins_matched-route-meta.yaml','546296aacb23b9a687559bbe2eae29f866443271afd30a4e0d38975ac995339e','Full1-24 local actual metadata plugin.'),
('MatchedRouteMeta/02_HTTPRoute_matched-route-meta.yaml','f8dd7f58cb95388c7555ead8207a0857334376832504d8c8f4e624cd9860200a','Full1-35 actual K8s route; EOF correction accepted.'),
('runtime/session_adapter.rs','54470cf128baa1a7510be8a49c1d35e70d5d680f540f43b39b09c0be8d0b7566','297-325,1080-1096 actual selectors/matchedroute owner.'),
('session_adapter_test/route_meta_tests.rs','4f8f85338bfc3cf2d0900b255dc4694a27838fca98fd71c9d516ea98030f0615','Full1-38 positive/missing/unknown selectors realunit.'),
('client/access_log_client.rs','f7691b8ad4c8765a0ab446c414a1596df48c930d8472678d724f918edcf84d55','1-95,334-412 trace normalization/strictretrieval/retries realunits.'),
('client/framework.rs','dd7b89a260befd5283e63c7a405e878918949ad17a618af0b23138312fed0383','100-140,196-241,608-825 URL/context/mappedwrapper/per-suite listener owner.'),
('client/test_client.rs','3f5afa96d10ac15f54b32df48c90af4e862cbf64a4942370f63540298eb80e0b','1565-1623,1768-1804,3015-3038,3840-3955 mapping/registry/invocation.'),
('integration-port-selection-must-fail-closed.md','c2ff61188030d30484afd0c1323201295fb980755c3e4738fe78dd8eea785b89','Full1-26 original listener contract.')],
'Same296 supplement request32 hardcodes31180 despite EdgionPlugins resolvedHTTP context, pendingindependentextension. Full both three-file fixtures/baseGateways/crossnamespacegrant/ports/templates/actual adapterctxmeta units/original runtime-template85/ctx-set60/claims53/accesslog/pglogging/logext/CtxSet read. Masking falsification: both harnessconfigs explicitly modeoff, productionreplace default retained; expected plaintext not defect. Original personalrules fresh/no nested. No execution/mutation/transportguarantee/broad absencecoverage, supports zero.')
persist('PART-502','/root/analyze_outbound',{
'mod.rs':('31c5f0f16340b7c7943be469da0ad3faa47a48c911ce0f10c85eae25fe23f6c9','Full1-3','Private export/registry accepted.'),
'openid_connect.rs':('a96d69cbca354098acdce6ffde01de4e89bd97bca60403dc92968aadd2a4517f','Full1-433','ARCH381 signature fixture encoding; other seven probes listener/introspection/scope/JWKS/Auto scopedcontracts retained.')},[
('openid_connect/token_verify.rs','9602a082d738a190f5df20b3d861d28aa4642d237d95d9cd32f5a02136a842b8','Full1-264 actual verify/Auto paths.'),
('openid_connect/error.rs','1ffb56f6ae20563e36425c8dc51e29615099c6ee8a0945e1cf47d47da470e810','1-100,195-315 decode-error/public projection; middle not claimedfull.'),
('openid_connect/plugin_test.rs','b898bea6c62fcd39ec46bf0efe62eb2805a013eb7b03a9f44b46ebc5a9a4b8c7','1728-1927,2092-2130 actual JWT/algorithm/scope/fallback units.'),
('server/test_server.rs','6c38b285bdfe354c40d5d2e96d4a07be0b7dc37ce3290197ca75054de23bfa98','4368-4515 actual OIDC/discovery/JWK/introspection; retained881-938 echo.'),
('OpenidConnect/03_EdgionPlugins_default_openid-connect-jwks.yaml','1178f083984e8740c732b00bce2c198199d3e324e3f9c546c698935d4bc3560e','Full1-25 localfixture.'),
('OpenidConnect/03_EdgionPlugins_default_openid-connect-jwks.yaml','a30363d55de3a218b0fc4f26f07b06c68bb5b7049e389e36c1d66ae59216be4d','Full1-25 K8sfixture.'),
('authentication-response-byte-caps.md','3eb35121ae9ce5a21c4524e819ec49ef3d3c919e5c465716ce31606e724d511b','Full1-105 original intended-branch fixture contract.'),
('dependency-error-projection.md','c8be1adab2bbcfb348f5cc38862efb7e7744bbdccd107574b5414f4bc6342ed2','Full1-110 original error classification/redaction.')],
'Static repositoryonly/no execution/freshdecoder behavior or parity. Fourteen direct rootfixtures bothtrees full+registry/Controller boundresolver/requestfilter/JWTcommon actualunitcontext; nestedCA/SAN suites notexpanded. Original personalroot/index/graded/routing full retained/freshunchanged/no nested. No decoder rejection observed or measured; supports zero.',candidate_ids=['ARCH-381'])
s=r.state();q=s.setdefault('pending_supplements',{}).setdefault('ARCH-296',[])
if 'REPORT-PART-501.json' not in q:q.append('REPORT-PART-501.json')
s['resume_dispatch']['analysis_outbound']=['PART-503','PART-504'];s['resume_dispatch']['analysis_conversion']=['PART-505','PART-506','PART-507'];s['resume_dispatch']['independent_verifier']='ARCH-377';r.save(s)
