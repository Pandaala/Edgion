# B0080 — HeaderCertAuth Exact Identity Selection

## Scope
- Baseline: `e6034b85`
- Pipeline state: committed and fully verified
- Active anchor crate: `edgion-gateway`
- Owned files: `edgion-gateway/src/plugins/http/header_cert_auth/{mod.rs,plugin.rs}`
- Exact implementation file: `edgion-gateway/src/plugins/http/header_cert_auth/plugin.rs`
- Necessary supporting file: `edgion-gateway/src/tls/runtime/backend/cert_extractor.rs`; this support edit does not claim TLS campaign coverage
- Expected no-change file: `header_cert_auth/mod.rs`
- Forbidden files: every existing `edgion-tests/**` path, resources/schema, session/runtime APIs, TLS callbacks, rustls-only code, access-log/metrics implementation, documentation, and protected user paths
- Exact future commit allow-list: the implementation and supporting files; campaign ledger/changelog/main record; B0079 and B0080 batch records

## Context and value gate
- Analysis subagent: `b0078_geoip_analysis`, revalidated against `e6034b85e2a898249894570b7406806d5625abee`; target blobs are `97b5b94b7fff9b192d6c61711d4a1a059ca875f2` for `mod.rs` and `0ed16b88c6bdd6497a96d94e0fd4ab9f1944dab6` for `plugin.rs`; supporting extractor blob is `1b00e2cd4d45ad31fa55d2d5ce663e21b71414a1`.
- Baseline tests: 8 HeaderCertAuth, 2 canonical extractor, and 3 resource tests passed.
- `ConsumerBy::Cn` and `SanOrCn` fell back to the full Subject DN when the documented identities were absent, authenticating an operator-unselected identity. Selected identity Strings were also cloned only to be borrowed for header insertion.
- Header mode duplicated the canonical subject/CN/SAN/fingerprint projection and used a more allocation-heavy fingerprint encoder.
- Decision: enforce exact SAN-to-CN, CN-only, and fingerprint identity selection; borrow the result; and share the canonical certificate projection after the existing Header-mode verification step.

## Hard-rule boundary
- Preserve Header-mode decoding, CA signature and validity verification order, Connection-mode handshake metadata, anonymous policy, fixed fail-close response, auth delay, credential hiding, header-control checks, and static log categories.
- Preserve `skipConsumerLookup` DN/first-SAN/fingerprint propagation. D0037/D0038 remain owners of cross-plugin identity-header cleanup and composition coverage.
- Preserve the supported BoringSSL Gateway boundary. Do not add or repair rustls behavior, cfgs, tests, or documentation.
- Add no tracing, metrics, access-log fields, sensitive values, schema behavior, dependencies, or integration-test edits.

## Implementation
- Complexity: complex; plan-first review required
- Implementer: `b0055_runtime_analysis`; plan-only phase completed before editing
- Plan reviewer: `b0080_plan_review`; approved after requiring explicit no-CN certificate fixtures plus full fail-close and anonymous runtime coverage
- Exact change: added a crate-visible X509Ref projection helper behind the unchanged public SslRef extractor, deleted the plugin-local projection, returned borrowed selected identities with no Subject fallback, and added real-certificate and full request regressions
- Test contract: exact canonical fields/fingerprint; pointer-identical SAN/CN/fingerprint borrows; subject-only prerequisites; 401 header/body/EOS/shutdown/log behavior; existing anonymous header order and success behavior

## Independent review
- Final code reviewer: `b0080_final_code_review`, distinct from analyst, implementer, and plan reviewer
- Verdict: approved with no must-fix or should-fix findings; confirmed projection equivalence, verification order, strict identity contract, fixtures, runtime branches, redaction, BoringSSL boundary, and deferred isolation
- Deferred adjudication: existing D0037/D0038 only; no new deferred or integration issue

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | protected external paths unchanged |
| Baseline focused tests | passed | 8 HeaderCertAuth, 2 canonical extractor, and 3 resource tests |
| Implementation and focused tests | passed | 12 HeaderCertAuth, 4 canonical extractor, 6 mTLS validation, 3 resource, and 34 PluginRuntime tests |
| Gateway all-target compile | passed | implementation and reviewer runs passed |
| Independent code review | passed | reviewer reran focused tests, Gateway Clippy, format, no-tracing, diff, and frozen checks |
| Full campaign gates | passed | format, workspace check, Clippy, agent docs, SSA, metrics, Pingora source, and no-tracing |
| Final frozen integration paths | passed | immutable integration-test baseline unchanged |

## Handoff
- Ledger/changelog updated: yes; 1 changed and 1 examined-no-change owned row; supporting TLS file is not claimed
- Commit: `332b2163`
