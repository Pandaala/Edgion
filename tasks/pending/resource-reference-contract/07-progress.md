# Execution and closure ledger

## Current resume snapshot (2026-10-10)

- Latest accepted local batches: R7a managed HTTP plugins 418e353dd,
  R7b internal HTTPRoute auth 78d0f168c, shared template readiness e954a9881,
  and R7c OAuth2 actual-owner transport df08e402b. Focused verification,
  workspace gates and two independent final reviews passed for each batch.
- The user added unrelated WAF commit a85aed2fb. R8a's reviewed baseline includes
  its seven new Gateway tests: 9637 total identities in the four tracked crates.
- R8a native Gateway TLS has passed both final execution reviews and all fourteen
  workspace gates. Its final scope is twenty technical/documentation paths plus
  this ledger. Verification passed 300 unit tests and 55 doctests, retaining ten
  existing ignored doctests and all 9637 baseline identities (9656 after nineteen
  additions). Committed locally as 67ae8580baafde977bc518773b05a7bb39e85893;
  all twenty-one committed blobs were verified. No push occurred.
- R8b BackendTLSPolicy actual-field/material pairing passed final execution
  review and fourteen checks (six refreshed after a one-expression test lint fix,
  eight reused with unchanged scope). There are 280 unit and 58 doc passes,
  ten existing ignored doctests, and 9671 identities with all 9656 baseline IDs
  preserved. Twenty-three technical/documentation paths plus this ledger are
  committed locally as e4cdf4c4e59195ee70af1a092487b2dcd0cf1abe after both
  independent Astra final reviews passed. All twenty-four committed blobs match
  the frozen manifest; no push or live/conformance claim is made.
- R8c health-local TLS plan r2 passed two independent Astra reviews at e4cdf4c4e.
  Twenty-five technical/documentation paths plus this coordinator-owned ledger
  now pass both independent static and execution reviews. The actual winning
  EBTP Arc replaces the detached active config, preserving generation fencing
  and invalid-check attachment. Verification passes 381 unit and 62 doc tests,
  with ten existing ignored docs; all 9671 old test identities remain among 9681.
  All fourteen root gates pass with zero warnings. Committed locally as
  4322b450532fc91a80d001fc33e85bace1186d7d after both final commit reviews;
  all twenty-six committed blobs match the frozen manifest. No push occurred.
- R8d EdgionTls plan r2 passed both independent Astra reviews at 4322b4505.
  Twenty-two paths plus this ledger now pass both independent static/execution
  reviews and fourteen root gates, with zero warnings. Server validation/LKG
  and present client-CA cold failure remain independent; attachment dimension
  violations still revoke admission. Verification passes 319 unit, 66 doc and 36
  schema tests, preserving ten ignored docs and all 9681 prior IDs among 9692.
  Committed locally as 700c29fba4adb43fd547d794c68654104e0193b0 after both
  final commit reviews; all twenty-three blobs match the frozen manifest.
  No push occurred; live verification is still open.
- R8e.1 ACME plan passed both independent Astra reviews at 700c29fba.
  Twenty-one technical paths plus this ledger now pass both independent static
  and execution reviews, with all fourteen refreshed root gates passing without
  warnings. Actual account/EAB reads, account-slot persistence/CAS identity and
  canonical HTTP-01 parent projection preserve existing operation fences and EAB
  authorization policy. Verification passes 328 unit, 71 doc and 36 schema cases;
  ten existing ignored docs and all 9692 prior IDs remain among 9702. Committed
  locally as 2aad70cdfc6eecf6e9d65daac90718c01f7b354f after both final reviews;
  all twenty-two committed blobs match the manifest. No push occurred.
  Neither ACME batch closes CredentialSource generated operations.
- R8e.2 generated certificate/EdgionTls destination plan passed both independent
  Astra reviews at 2aad70cdf. Twelve technical paths plus this ledger now pass both
  independent static/execution reviews and all fourteen root gates, zero warnings;
  all active output identities are checked before output IO, and existing attempt
  fences, adoption, forced SSA and nontransactional publication remain unchanged.
  Verification passes 259 unit, 74 doc and 36 schema cases, preserving ten ignored
  docs and all 9702 prior identities among 9704. Committed locally as
  b896e5c661a52f60d7af1a81f4b8718df3c938b4 after both final commit reviews;
  all thirteen committed blobs match the manifest. No push occurred.
- R8f.1 managed credential projection at b896e5c66 passes both independent
  Astra static/execution reviews and all fourteen root gates, zero warnings.
  Fifteen technical paths plus this ledger remove duplicate output coordinates,
  bind consumer/source pairs and check qualified material identity before decode.
  Historical cleanup remains independent of adoption. Verification passes 323
  unit, 74 doc and 36 schema cases (433 distinct), retaining ten ignored docs
  and all 9704 old identities among 9709. The added generation-only negative
  control passes; the initial count regression and its correction remain in the
  evidence. Committed locally as dc9be9a7fc8c1ee7fdd910a98d3a5e6e9552e19c
  after both final commit reviews; all sixteen committed blobs match the manifest.
  No push occurred.
- R8f.2 managed Secret API identity at dc9be9a7f passes both independent Astra
  static/execution reviews and all fourteen root gates with zero warnings. Eight
  technical paths plus this ledger derive API coordinates from checked current
  pairs or observed historical cleanup identity and validate returned metadata
  before reuse. POST/409, CAS, UID/RV deletion and late authorization remain.
  Verification passes 254 unit and 74 doc tests (328 distinct), with sixteen
  existing ignored docs; all 9709 old identities remain among 9714. The runner's
  nonzero-pass assertion after successful ignored-only Controller docs is retained
  transparently; remaining Resources docs passed separately. Committed locally as
  4b920e3c3e448b60c59e6c6cad608c1e7d97f94e after both final commit reviews;
  all nine blobs match the frozen manifest. No push occurred. Live API and
  conformance verification remain open.
- R9a Backend carried Secret pairing at 4b920e3c3 passes both independent Astra
  static/execution reviews and all fourteen root gates, zero warnings. Twelve
  technical paths plus this ledger include six metadata-only fixture additions
  identified during plan review and one explicitly approved malformed-owner test
  expectation change. Verification passes 265 unit, 76 doc and one feature-enabled
  H1/H2 standalone transport test (342 distinct), preserving ten ignored docs and
  all 9714 old ordinary test identities among 9718. Transport is excluded from
  that ordinary identity count. Committed locally as
  4737874343ac32c3ca4b963e6406535489966a65 after both final commit reviews;
  all thirteen blobs match the frozen manifest. No push occurred.
- R9b LinkSys non-TLS auth at 473787434 passes both independent Astra static and
  execution reviews plus all fourteen root gates with zero warnings. Seventeen
  technical paths plus this ledger unify cold auth checks across Common and both
  direct OTLP parsers. Ordinary same-type NotReady retention remains; forced
  replacement before that match, deletion and type changes are unchanged.
  Verification passes 566 unit, 78 doc and 10 actual standalone OTLP transport
  cases (654 distinct), with ten existing ignored docs. External Redis is strictly
  compile/list-only: three ignored, zero executed. All 9718 old ordinary identities
  remain among 9726; 126 old bodies retain approved fixture adaptations only.
  The r1 token-lifetime compiler failure and r3 isolated stale-binding controls
  remain in evidence; unaffected r2 reuse is explicit. Committed locally as
  eac49a77bb1e6d7e0cf65139dd0d26dac929f009 after both final commit reviews;
  all eighteen blobs match the frozen manifest. No push occurred. Broad live,
  admission and conformance verification remain open.
- R10 obsolete detached reference API cleanup at eac49a77b passes both independent
  Astra static/execution reviews and all fourteen refreshed root gates, zero
  warnings. Twenty-two technical paths plus this ledger remove unused detached
  entry points while preserving live canonical consumers and dependency ordering.
  The warning correction includes two independently approved test-only paths;
  initial fixture failures and workspace warning logs remain in the evidence.
  Verification passes 542 unit and 78 doc tests (620 distinct), with ten ignored
  docs and 31 duplicate executions excluded. All 9726 prior test identities remain;
  204 old tests retain only their approved canonical migrations and key fixtures.
  Committed locally as e52229052acc79331fced0e0dd0c3eaf43792f2f after both
  final commit reviews; all twenty-three blobs match the frozen manifest.
  No push occurred. C1/C2/C3 and live/conformance remain open.
- R11a native Gateway backend client pairing at e52229052 passes both independent
  Astra static/execution reviews and all fourteen root gates with zero warnings.
  Twelve technical paths plus this ledger bind the actual declaration and carried
  material before cold store publication. Ordinary peers and AllEndpointStatus
  probes retain their readers, BTP precedence, native defaults and finite raw-default
  handling; no request-path owner lookup or Grant policy change was introduced.
  Verification passes 250 unit and 81 doc tests (331 distinct), ten ignored docs;
  all 9726 prior identities remain among 9731. Twenty-five old scoped tests retain
  only the two approved fixture migrations. Three new global-state tests use the
  existing exact-child isolation helper. Positive compilation and raw E0505/E0506
  diagnostics verify the material loan. Initial test-only compiler failures remain
  in evidence. Committed locally as 42251846dfa782817b9b1b479e34f9f818486cf3
  after both final commit reviews; all thirteen blobs match the frozen manifest.
  No push occurred. C1/C2 and live verification remain.
- R11b HeaderCertAuth CA pairing at 42251846 passes both independent Astra
  static/execution reviews and all fourteen root gates with zero warnings. Nine
  technical paths plus this ledger pair compressed material with actual declared
  targets before parsing/cache reuse, preserving partial-good CAs, Connection
  inactivity and existing crypto/auth outcomes. Reference changes still create a
  fresh generation-owned cache; no second cache identity mechanism was added.
  Verification passes 367 unit and 84 doc tests (451 distinct), ten ignored docs;
  all 9731 old identities remain among 9734. All 102 old scoped tests remain with
  four approved body fixture migrations and two helper adaptations. Actual request
  controls prove retarget/restoration, CA rotation and schema-error priority.
  E0505/E0506 diagnostics verify owner loans; the standalone negative drop probe's
  unused-mut warning is separate from warning-free Cargo/root phases. Initial
  E0597 and runner-path startup errors remain documented. Committed locally as
  6aee23ed423e1d89989b519b4d3c026fe5c77446 after both final commit reviews;
  all ten blobs match the manifest. No push occurred. C2 local TLS consumers and
  broad live/conformance verification remain.
- R11c.1 OIDC actual local TLS binding at 6aee23ed passes both independent
  Astra static and execution reviews. The seven technical paths plus this ledger
  bind ordinary/SAN construction to the actual entry and one captured global
  snapshot before cache reuse, preserving error precedence and transport policy.
  Focused verification passes 711 distinct cases (613 unit and 98 doctests),
  retaining ten ignored docs and sixteen ignored OIDC cases. All 9734 baseline
  IDs remain among 9738; exactly eight reviewed old fixture migrations preserve
  assertions. Initial compile and detached-dye-fixture failures remain recorded.
  All fourteen root gates pass with zero warnings (workspace check 26.379 seconds,
  Clippy 37.195 seconds). Source/log/binary evidence is under
  `tmp/resource-reference-contract/r11c1-verification-20261010/` and root gates
  under `tmp/resource-reference-contract/r11c1-root-verification-20261010/`.
  Committed locally as 4d3cd897f0bf1f32638fe5be8b8a5e65d2a9fcec after both
  final commit reviews; all eight blobs match the manifest. No push or live/
  conformance acceptance is claimed.
  LDAP, ExtProc and Wasm remain separate serial batches; Wasm must explicitly
  fix disabled-custom-TLS behavior while preserving HTTPS. Whole-contract review,
  documentation/task cleanup and fresh live/conformance verification remain open.
- R11c.2 LDAP actual-entry TLS binding at 4d3cd897 passed dual plan/static/
  execution review; both reviewers also approved the final test-only correction.
  All transports validate their actual owner; only LDAPS/StartTLS consume local
  TLS proof and captured inheritance before cache/construction. Plain LDAP ignores
  inactive TLS material/defaults; egress, SAN/M4 and authentication remain intact.
  Focused evidence passes 587 distinct cases (489 unit, 98 doctests), retaining
  ten ignored docs, all 9738 prior IDs among 9741, and 93 scoped old identities/
  attributes. Sixteen constructor expressions and two malformed-material fixtures
  migrate without weakening old assertions. Initial two test compile errors and
  three Clippy warnings are preserved. The final three-line test-only correction
  passes LDAP 28/28; unchanged-phase reuse is explicit in verification-final.json
  under `tmp/resource-reference-contract/r11c2-verification-20261010/`.
  All fourteen refreshed root gates pass without warnings (check 14.468 seconds,
  Clippy 21.788 seconds). Committed locally as
  f8ea3897cd42be879154a4285b66e12b3ab76e81 after both final reviews; all six
  committed blobs match the manifest. No push or fresh live/conformance acceptance
  is claimed. ExtProc and Wasm remain.
- R11c.3 ExtProc actual-entry TLS binding at f8ea3897 passes dual independent
  Astra plan, final static and execution review. Source/material checks precede
  both entry and target-cache reuse, and request/response companions share one
  captured global snapshot. TLS-disabled transport ignores inactive TLS policy;
  capability checks, fingerprints, lazy dial and streaming semantics remain intact.
  Focused verification passes 724 distinct cases (626 unit and 98 doctests),
  retaining ten ignored docs, all 9741 prior IDs among 9744, and 127 scoped old
  identities/attributes. The approved caller/helper migrations and two metadata
  fixtures preserve old assertions. The initial new-test fingerprint expectation
  failure remains recorded; r2 preserves the existing full validation fingerprint
  and distinguishes owner/timeout reuse from CA kind/name retarget or rotation.
  Both new companion assertions use the async collection. Evidence and explicit
  unchanged Resources/Common reuse are under
  `tmp/resource-reference-contract/r11c3-verification-20261010/`.
  All fourteen root gates pass without warnings (check 57.583 seconds, Clippy
  60.761 seconds). Committed locally as
  fed9aa66ec92b4babae292b7b0c2dcc504089db9 after both final commit reviews;
  all ten committed blobs match the manifest. No push or fresh live/conformance
  acceptance is claimed. Wasm is the remaining C2 family.
- R11c.4 Wasm at fed9aa66 passes two fresh independent Astra final static and
  execution reviews. Eight technical paths plus this ledger bind all three
  supported stages before cache/private runtime/preload and carry one prepared
  URL client result. Identity errors reject construction; ordinary crypto errors
  retain module-load negative-cache/failOpen semantics. Absent/disabled custom
  TLS remains inert while native HTTPS, OCI, hostcallout and task ownership stay
  intact. Focused verification passes 798 distinct cases (700 unit, 98 doctests),
  with ten ignored docs, all 9744 prior identities among 9749, 168 old scoped tests
  and 205 old function bodies preserved through approved constructor plumbing.
  Initial new-test failures, runner setup error and one Clippy needless-borrow
  warning remain recorded. R4 removes only that redundant test-helper borrow;
  Wasm 227/227 passes again, with explicit unaffected-r3 reuse and old binary
  provenance retained in verification-final.json under
  `tmp/resource-reference-contract/r11c4-verification-20261010/`.
  All fourteen refreshed root gates pass with zero warnings (check 15.645 seconds,
  Clippy 22.535 seconds), with unchanged frozen source. Root evidence is under
  `tmp/resource-reference-contract/r11c4-root-verification-20261010/`.
  Local commit review is pending. Whole-contract review, documentation/task
  cleanup and fresh live/admission/conformance acceptance remain open; no push.
- F1's Gateway EdgionBackend and Common LinkSys non-TLS auth material-pairing
  candidates are closed by R9a/R9b. Existing Controller guards remain; these are
  consumer proof boundaries, not demonstrated ingress exploits. Generated CredentialSource operation identity is closed by R8f.1/R8f.2
  and ACME output identity by R8e.2.
- Final whole-contract audit at eac49a77 found three additional finite cold
  consumer proof gaps: native Gateway backend client material (ordinary peers
  and AllEndpointStatus probes), HeaderCertAuth carried CA material, and local
  TLS in OIDC/LDAP/ExtProc/Wasm artifact fetch. Normal bound Controller acquisition
  remains a positive guard; these are not reproduced ordinary Grant bypasses.
  R11a/R11b close the Gateway backend client and HeaderCertAuth consumer gaps
  after scoped verification.
  Independent cross-direction review confirmed the Gateway and HeaderCertAuth
  paths. Grant/native source checks found no additional confirmed matcher/default
  defect; broader live verification remains open. Close each consumer family
  through its own reviewed batch after the independently scoped R10 cleanup.
- Native Gateway material selection preserves omitted raw namespaces without
  relaxing native group/kind or Grant semantics. Default's candidate reads are not
  atomic; registered dependencies and subsequent source events reconcile changes.
- Remaining R8 consumers, full coverage/simplification review, knowledge and user
  documentation cleanup, fresh real-environment admission/dynamic/conformance
  verification, and original issue closure are open. Scoped passes do not prove
  whole-task completion.
- No agent push. Protected unrelated untracked paths: review-results/ and
  tasks/todo/resource-reference-abstraction-simplification.md. Exclude them from
  batch commits unless separately assigned.

The entries below retain historical checkpoints; older pending labels describe
those checkpoints and are superseded by later accepted batch records.

## R8c final checkpoint (2026-10-10)

The reviewed twenty-five-path r2 plan binds actual EBTP active TLS fields through
the existing canonical reference, material and acquisition mechanisms. Default
bare/qualified source recovery extends only to the finite health consumer;
same-namespace restrictions and ordinary Secret/ConfigMap provenance remain.
The winning policy Arc replaces the detached health configuration. TLS material
is proved once at task startup; endpoint pinning, independent TLS defaults,
invalid-winner attachment and per-poll generation fencing remain unchanged.

Both Astra reviewers approved production and final execution. One new test was
strengthened after review to isolate name, namespace and UID generation changes;
only that test file changed from r1 to r2. Its 45-test health suite passed again.
Other results retain their explicit unchanged-input r1 evidence: 381 distinct
unit and 62 doc passes in total, ten existing doc ignores, zero warnings. The
9671 baseline identities and ignored sets are preserved, with ten additions.
Lifetime positive control and intended E0505/E0506/E0502 negatives passed.
Old fixture adaptations are limited to the approved actual-owner construction
and borrowed accessor changes; Controller old test files remain exact prefixes.

Root format, workspace check (40.354 seconds), Clippy (48.954 seconds), docs,
SSA, metrics, Pingora source, transform/reference contracts, test layout,
Gateway tracing, English and diff checks all pass on unchanged source. Evidence:
`tmp/resource-reference-contract/r8c-verification-20261010/verification-final.json`
and `tmp/resource-reference-contract/r8c-root-verification-20261010/`.
The final commit manifest adds only this ledger to the twenty-five approved paths.
No live TLS, integration or conformance acceptance is implied. No agent push.

The read-only F1 inventory and F2 execution runbook are saved under
`tmp/resource-reference-contract/f1-coverage-e4cdf4c4-20261010.md` and
`tmp/resource-reference-contract/f2-preflight-20261010/F2-RUNBOOK.md`.
EdgionTls, ACME/generated output, the bounded AI/LinkSys material-pairing
candidates, overall simplification/documentation and fresh live verification
remain open; the original issue must not be closed yet.

## R8d final checkpoint (2026-10-10)

Actual EdgionTls server and client-CA fields independently use shared strict
identity, ordinary Secret provenance, bound acquisition and readonly status.
Literal core remains unsupported. Omitted namespace uses the finite default
source selector and Secret event recovery. The existing disabled-Grant exception
is limited to EdgionTls; invalid references gain no material dependency or read.
No wire/schema fields, handler IO, queues or handshake-time checks were added.

Server proof runs before certificate bytes are consumed in Common and direct
Gateway cold compilation. Present client-CA proof stays in cold CA compilation:
if all other validate_cert checks succeed, invalid CA material publishes the new
failing trust state instead of retaining old trust. Existing validate_cert
failures retain their whole-generation same-key LKG policy, including UID changes,
while invalid attachment dimensions revoke admission. Terminate ignores CA use.

Both independent Astra reviewers approved final static and execution evidence.
One preliminary new-test import error was corrected without changing visibility.
The first Secret event run exposed an incorrect expected UID after a qualified
source deletion; the corrected test requires the surviving bare source's actual
UID. The failed log is retained and the corrected thirteen-test suite passes.
Reviewed documentation corrections identify the true crypto/status owner and
all prerequisites for new-generation client-CA failure. Final r3 changes only
one documentation capitalization from r2; explicit unchanged-input reuse is
recorded, without counting repeat runs as additional distinct tests.

Verification passes 319 unit, 66 doc and 36 schema cases (421 distinct), with ten
existing ignored docs and zero warnings. All 9681 prior test identities and
ignored sets remain; eleven additions produce 9692. Actual executable hashes,
approved old fixture adaptations and positive/intended E0505/E0506/E0502 lifetime
controls were independently verified. Fourteen root gates pass unchanged source;
workspace check took 35.592 seconds and Clippy 43.056 seconds. Final evidence is
`tmp/resource-reference-contract/r8d-verification-20261010/verification-final.json`
and `tmp/resource-reference-contract/r8d-root-verification-20261010/`.

The final manifest includes exactly twenty-two approved paths plus this ledger.
No live TLS, live Grant-only reconciliation or conformance acceptance is claimed.
ACME candidate inventory is saved as
`tmp/resource-reference-contract/r8e-acme-inventory-4322b450-20261010.md`;
ACME/generated outputs, AI/LinkSys consumer pairing, overall cleanup and F2 remain
open. No agent push.

## R8e.1 final checkpoint (2026-10-10)

Actual ACME account/EAB field types enforce strict identity and prevent EAB from
entering account persistence. API responses require explicit target metadata
before bytes or UID/RV reuse; raw default cache projection remains finite and
separate. Account conflict/adoption/CAS, attempt/leader fences, EAB authority and
HTTP-01 selection are preserved. Generated certificate/EdgionTls outputs remain
R8e.2 work; CredentialSource output identity is a separate backlog.

Both Astra reviewers approved final r4 static/execution evidence: 435 distinct
passes (328 unit, 71 doc, 36 schema), ten ignored docs, zero warnings, all 9692
prior identities preserved with ten additions. Positive lifetime and intended
E0505/E0506/E0502/E0308 failures prove borrowing and account/EAB role separation.
Review strengthened actual processed account-status event transitions and the
positive forced-SSA account-create transport control. The initial new Gateway
fixture failure, preliminary compile diagnostics and two Clippy test warnings
are retained; corrected service 35 and Secret 14 reruns pass. Reuse follows exact
unchanged production/test inputs, not a claim that every phase ran at r4.

All fourteen root checks were rerun after the mechanical lint fixes and pass on
unchanged source; workspace check took 8.883 seconds, Clippy 12.537 seconds.
Evidence: `tmp/resource-reference-contract/r8e-verification-20261010/verification-final.json`
and `tmp/resource-reference-contract/r8e-root-verification-20261010/` (r2 root
results). Final scope is the twenty-one approved paths plus this ledger. No live
API-server, CA or Gateway API conformance acceptance is implied; no agent push.
The separate generated-credential inventory is preserved under
`tmp/resource-reference-contract/generated-credential-inventory-700c29fb-20261010.md`.

## R8e.2 final checkpoint (2026-10-10)

The actual ACME output view validates all active destinations before output IO;
helpers derive only from the retained attempt owner. Explicit returned target
metadata is checked before optional UID/RV reuse. Source/storage namespaces,
independent auto-TLS parents, forced SSA, adoption, operation fences and existing
nontransactional publication remain. Unchecked output helper methods are removed.

Both Astra reviewers approved r2 static/execution evidence. Review strengthened
the new transport matrix to assert exact ordered GET/PATCH paths; production
remained unchanged. All prior assertions are preserved except the approved two
Resources helper expressions, with expected values and test identity unchanged.
Verification passes 369 distinct cases (259 unit, 74 doc, 36 schema), ten ignored
docs and zero warnings. All 9702 old test IDs remain among 9704. Matching executable
and log hashes, prior baseline provenance, explicit unchanged-input reuse, and
positive/E0505/E0506 output lifetime controls were independently checked.

All fourteen root gates pass unchanged source, including workspace check
24.516 seconds and Clippy 34.182 seconds. Evidence is under
`tmp/resource-reference-contract/r8e2-verification-20261010/verification-final.json`
and `tmp/resource-reference-contract/r8e2-root-verification-20261010/`.
Final scope is twelve approved paths plus this ledger. No live CA/API-server or
conformance acceptance is implied. Generated CredentialSource operations,
AI/LinkSys consumer pairing, overall simplification/documentation and F2 remain
open. No agent push.

## Authority and reporting

The user now requires root coordination, delegated IMPL implementation and a local
commit for the current verified checkpoint and every subsequent completed batch.
Never push. Keep exactly one mutation lane; read-only inventory may overlap.
Each implementer reads original rules and knowledge, returns a plan before edits,
obtains independent Astra plan approval, implements and verifies, and obtains a
fresh independent Astra result review. Material deviations require renewed plan
review. Root checks the diff, gates, staged scope and commit result.

C0 checkpoint reviewer capacity note: creation briefly returned `agent thread limit
reached`. Reusing existing agents restored separate implementation, plan review
and fresh result review roles. C0's fresh result reviewer's runtime model
is unknown and that checkpoint outcome must be described as independent review,
never as an Astra PASS. R1/R2 use confirmed Astra reviewers. The confirmed Astra
checkpoint audit/plan review and required later
multiple confirmed Astra whole-task reviews remain distinct gates.

R2c reviewer-capacity note: two attempts to revive the separate historical
checkpoint Astra role returned `agent thread limit reached`. The current
namespace inventory agent therefore becomes the fresh independent R2c result
reviewer; it did not author, approve, or implement the R2c plan. Its model is
unconfirmed, so its verdict must not be labeled Astra. After that fresh review,
the confirmed Astra R2c plan reviewer performs an additional deep actual-diff
result round. Record both roles and outcomes separately; require both passes,
and retain the requested confirmed Astra whole-task rounds at closure.

R2c runtime recovery supersedes that active-role allocation: after a later turn
interruption the former reviewer threads disappeared from the live roster and
the original `/tmp/edgion-ref-r2c-*` artifacts were absent. Root explicitly spawned
two new reviewers with `model=gpt-6-astra`: `/root/r2c_fresh_result_astra` is the
fresh independent final reviewer and `/root/r2c_deep_result_astra` performs the
subsequent deep result round. Neither authored or implemented this batch. Both
must review the frozen actual result and pass before commit; preparation is not
a result verdict. The existing implementer remains the only mutation lane.

The original issue remains open. Do not report total completion from scoped tests
or historical conformance. Record commit hashes only after successful commits.

## Current evidence

- The finite reference contracts, source-schema projection and actual-owner
  binding foundation are implemented; consumer migration remains incomplete.
- Bound credential acquisition, exact Grant queries/revisions, ConfigMap CA
  provenance, plugin credential stages, conditions, templates, descriptor
  ConfigMaps and EGC administrative TLS selection have scoped implementation
  evidence. Generic dependency/status and downstream consumers remain open.
- Seven consumers have scoped independent review acceptance: HTTPDNS, Webhook,
  Redis, Elasticsearch, Kafka, Etcd and OTLP. Detailed original reviews and fresh
  test logs are recorded in 04-implementation.md. Their completion is not proof
  of Basic/auth Secret authority or every downstream consumer.
- The native five-Route backend/Grant implementation has fresh independent Astra
  scoped PASS and warning-free workspace gates. Its local commit is
  0fea0a618cec55d740719e4bbbb433c3c1438c11. No push occurred.
- HTTP/GRPC final backend selection, SP, DirectEndpoint and policy/TLS lookup
  consume cold owning-Route targets in R2a, with fresh independent Astra scoped
  PASS and warning-free workspace gates. Its local commit is
  d37b890a8b448710d78fab5d587a2a5306e4b8cc. Post-commit status contained only
  unrelated untracked review-results, left untouched. R2b implementation at this
  HEAD passed focused tests, final workspace gates and a fresh independent Astra
  scoped result review. R2b was committed locally as
  5cf413bbc4e4b4b4a28be378e3994137779fc2d8; no push occurred. Post-commit status
  contained only unrelated review-results. R2c's effective 32-path implementation
  passed two new independent Astra result rounds, fresh scoped/live verification
  and all workspace gates, then committed locally as
  f65b863c7cda6ec9936569d91e7bd8897824c4b2. Post-commit status contained only
  unrelated review-results; no push occurred. R3a1 ExtensionRef Resources/Controller
  passed both Astra final reviews and warning-free gates, then committed locally
  as 05e7debf53e87ab3709e7d2090c0b3e7356244ca. Post-commit status again contained
  only unrelated review-results; no push occurred. Gateway R3a2 passed both corrected-result Astra reviews, stable tests and
  warning-free gates, then committed locally as
  75a5363f52275a7e48252308f1f351c4a51504a7. Its test-only Mutex finding is
  resolved. Post-commit tracked files and index were clean; no push occurred.
  R3b mirror migration passed both independent Astra result rounds and all
  warning-free root gates, then committed locally as
  16e7be6dce9d9d7ffc9dd70fa6889a557043aace. No push occurred. R3c descriptor
  migration completed corrected implementation, two independent actual Astra
  review passes and clean renewed root gates, then committed locally as
  ca7bdaf79bed40287505287345e5ddb5f443ae84. R4a subsequently passed both
  independent Astra reviews and clean gates, then committed locally as
  f8bfbf6a52aa509db50366fe3f6521e931d56835. R4b then passed both independent
  actual reviews and clean gates, committing5225c3e15d9e2eec5c6448935d7a226326b9b9e6
  locally. No push occurred. R4c is in plan preparation; remaining R4-R8 and
  F1/F2 remain pending.
- The first checkpoint audit and workspace commit gates passed. The user
  committed the accumulated code snapshot
  as 0f40e2708496e9aacafb0dc2fbd2cdd786ffa9e8 (`tmp commit`) while verification
  was running. The coordinator completed C0 in local closing commit
  8dab81bc69cd33094e12962809bb3c0cca7e1c19, including the final test correction,
  documentation and execution records. No push occurred. The user's commit
  remains unchanged.
- The earlier external baseline was 4dbcc5495; current HEAD is the completed
  R4b commit5225c3e15. Revalidate each batch against its current HEAD.
  The independently owned, untracked `review-results/` artifacts remain outside
  this task's mutation and staging scope.

## Completed checkpoints and retained intermediate evidence

Independent Astra audit found two immediate corrections: remove the newly added
gRPC temporary identity allocation in `backends/mod.rs`, and eliminate bare
production `unwrap` in Common global TLS client selection. The proposed minimal
IMPL correction additionally resolves ten introduced Clippy warnings and corrects
the old EGC Grant knowledge claim. Independent Astra plan review returned scoped
PASS, including a private borrowed gRPC context predicate and table tests in the
existing mirrored backend test file. The implementation subagent is authorized
to execute this frozen correction scope. The native Route plan remains the next
batch after commit.

Fresh pre-correction workspace check and Clippy exit zero; Clippy reports three
Resources production, six Resources test and one Controller test warning. These
are not warning-free acceptance. Formatting, diff, docs, SSA, metrics, Pingora,
transform and reference drift, three reference generator regression tests,
unit-test layout (997), Gateway tracing and English guards pass. Log prefix:
`/tmp/edgion-ref-checkpoint-`. CRD coverage passes 28 tests and plugin condition
coverage passes eight tests.

The correction subagent has finished its approved fourteen-file scope, including
the separately reviewed condition mirror and gRPC matrix amendments. Fresh focused
results pass: Resources references 67 and doctests 19/ten existing ignored,
Common global TLS seven, Gateway backend guard three and gRPC routes 66,
Controller credential manager 50 and EGC handlers 18. Failed initial Resources
and gRPC logs are retained; no test identity was removed in this corrective slice.
Final formatting and doc checks pass. Full workspace check-r2 and clippy-r2 exit
zero with no warnings/errors. The original-user snapshot is preserved. The fresh
independent result reviewer returned C0 scoped PASS after reproducing the frozen
279-path digest and checking the cumulative base-to-user-commit plus closing
worktree. Its runtime model is unknown and this is not labeled an Astra PASS.
All final r2/r3 guards pass; unchanged CRD and generator inputs retain their passed
28/eight and three-test evidence. The coordinator's closing commit is complete.
The next native Route implementer is revalidating the concrete plan against
that commit without production edits until independent plan approval.
Full-manifest/conformance closure remains open.

R1's concrete ten-part plan and amendments A1-A5 received independent Astra
scoped PASS from `/root/checkpoint_astra_audit`: thirteen production paths,
twelve mirrored test paths and seven documentation paths. The delegated
implementer is now authorized to execute that frozen scope. Invalid owners use
this controller's Accepted=False and empty derived execution content; invalid
children are rejected before clone while valid named/wildcard recovery edges
remain. Denied backend status and AI facts do not read new targets. Necessary
cleanup of old valid dependencies is distinct from prohibited invalid-target
discovery. A separately reserved fresh Astra result reviewer has loaded the
original rules and scope but has not issued an implementation verdict. R2
inventory may proceed read-only; its plan review and implementation wait for the
R1 commit gate.

F1 documentation follow-up: `11-reference-grant.md` still describes EGC selection
as a Grant reference with an empty source namespace. Reconcile that relationship
table with the current administrative selection contract; it is outside R1's
frozen documentation scope.

F2 read-only environment preflight found working Docker and Kubernetes tools and
a Ready OrbStack node. Its shared cluster already contains Edgion and unrelated
workloads; the running Edgion images are historical dynamic-parity artifacts.
Their readiness is not current verification. Before final admission/conformance,
prepare an isolated test deployment with freshly built Controller/Gateway
artifacts and explicit CRD channels rather than mutating existing shared tests.

Read-only R2 inventory identified shared cold Service targets, native context
fallbacks, L4 invalid-port reselection, and unreachable standard-Service
ExternalName dispatch. Tentative serial splits are HTTP/GRPC final selection,
L4 compiled slots, then coherent Service transport/DNS consumption. Each needs
its own concrete reviewed plan and commit. Preserve custom plugin outbound field
contracts until their actual-owner migration; do not remove a shared fallback
without proving every caller's replacement.

Read-only R3 inventory identified detached ExtensionRef compilation, invalid
runtime lookups and per-request key formatting across runtime, CORS and access
logs; mirror still needs actual-owner authorization and disabled-entry handling.
Descriptor adapters already bind actual EdgionPlugins owners, but their complete
ConfigMap event-to-runtime repair chain needs verification. Tentative separate
plans are ExtensionRef, mirror, and descriptor lifecycle coverage. The mirror
plan must preserve status-only missing-versus-denied classification, distinct
from authorization to consume the target, using current vendored semantics.
Neither inventory authorizes implementation; revalidate overlapping files after
the R1 commit.

R3 revalidation at 5cf413bbc confirms R1 already clears authored ExtensionRef
`resolved_namespace` before flattening in both HTTP/GRPC handlers; do not repeat
that historical repair. Current PluginRuntime still records unsupported
ExtensionRef keys, allowing global runtime/access-log consumers to query before
the filter's late supported check. The filter's own cold lookup key already
avoids stage-time formatting, but snapshot/access-log consumers remain separate
call-site gaps. This is read-only evidence; exact R3 plans remain unapproved.

Independent Astra mirror-authority adjudication at 5cf413bbc corrected the
inventory's inferred blanket conflict. Spec sections 5 and 7 require identity
validation, recovery dependencies, exact authorization and matching consumption;
they do not prohibit every denied Controller presence-only diagnostic. Standard
v1.6.2 CRD lines 8196-8205 distinguish an absent referent from an existing denied
object. Actual mirror Grant evaluation already precedes `errors`, whose
`service_exists` returns only a cached boolean for status. Preserve the existing
denied-plus-missing `BackendNotFound` assertion and existing-denied
`RefNotPermitted`, while actual-owner loans replace detached identity and the
runtime denial still prevents endpoint/body/writer/dial consumption. Native
terminal-backend status rules are not automatically MirrorCore rules. This
requirements adjudication is not R3 plan approval or current mirror acceptance;
invalid-owner/disabled-entry/dependency gaps remain to implement and verify.

Read-only R4 inventory at 0fea0a618 found one concrete remaining status migration:
`EdgionPluginsHandler::resolve_secret_refs` rebuilds Grant and material targets
from reverse dependency strings rather than current field tokens. Normal parse
clears those indexes; this evidence does not establish a normal-path credential
bypass. Existing bound credential, TLS, Wasm, descriptor and condition walkers
cover current fields, but CredentialInjector deliberately suppresses Secret
missing/denied errors because the old scan supplies them. A tentative R4a must
remove both dependencies together, preserve exact deterministic dedup, and retain
persist-off's absence of generated-Secret lookup/dependency. Preserve the two
existing Injector test identities while replacing their old suppression
assertions with token errors and single handler-level outcomes. Tentative R4b
adds actual Secret/ConfigMap event-to-current-generation lifecycle evidence,
without rewriting apparently correct production or duplicating R3 descriptor
coverage. The EP architecture and original Wasm/descriptor knowledge limitations
need same-batch updates. Raw pipeline callers outside EP and CredentialSource
async publication remain R7/R8 work. Neither candidate has an approved plan or
fresh runtime verification; revalidate after R2/R3 commits.

Supplementary namespace inventory at d37b890a8 confirms that absent source
namespace remains a bare processing/wakeup key, while reference scope defaults
to `default`. FS and etcd ingestion preserve metadata and reject storage-key
mismatches; these paths do not authorize global namespace normalization. Bound
credential and Grant recovery preserve the distinction. R4 lifecycle tests
should exercise an actual namespace-less EP owner through target/Grant events,
queue processing, generation replacement and material/status repair. Existing
descriptor and bare-key tests do not prove the complete event chain. This is
read-only evidence, not a new runtime PASS or R2b scope expansion.

Outside this task, the inventory identified a deferred candidate: namespaced
Admin create/update permits omitted body namespace without supplying the URL
namespace, but FS/etcd ingest subsequently requires matching resource identity.
The related Admin knowledge claim that omission defers to the URL needs
independent confirmation against an actual write/read scenario. No Admin,
filename, or global metadata change is authorized by this reference batch.

Read-only R5 inventory at 0fea0a618 identified tentative independent boundaries:
GatewayClass parameters, parent/allowedRoutes attachments, policy target owners,
and three ConfigData consumer groups (EP/conditions/RAP, Stream, WAF/ProxyProtocol).
Parameters currently differ between Controller, listener construction and plugin
publication: the first ignores namespace and the second ignores group/kind too,
while publication enforces the cluster target. Two vendored CRD channels require
cluster namespace unset. Policy target classification is already shared, but
detached namespace arguments and Controller default versus Gateway empty-string
owner handling remain. Parent comparisons, consent, UID/class incarnation gates,
dropped-parent recovery and unsupported default-Gateway behavior must survive
the actual-field migration. ConfigData has established Cluster-visibility Grant
exemption requiring target visibility reads; it must not inherit Secret lookup
ordering. Disabled EP/Stream ConfigData walkers still create edges/read targets.
WAF bundle authorization comes from the actual policy ConfigData; the EP wakeup
owner cannot replace it. None of these candidates is approved implementation or
runtime evidence; revalidate overlapping publication/status paths after R2/R3/R4.

R2a concrete plan received independent Astra scoped PASS from
`/root/checkpoint_astra_audit` after original-source review and explicit factory,
policy-capture and error-table amendments. Frozen scope: 51 paths (19 production,
23 test/support, nine documentation), plus this coordinator-owned ledger.
Baseline copies, hashes and 347 original source test identities are under
`/tmp/edgion-ref-r2a-before-0fea0a618`. Root authorized delegated serial execution;
`/root/native_route_astra_result` is the separate fresh Astra result reviewer.

Required R2a chain: actual HTTP/GRPC owner and effective rule index to target-only
loan, sealed cold Service target, original slot handle, selected proof and final
peer. SP, DirectEndpoint and policy/TLS capture borrow the same preformatted key,
checked section and policy identity. Invalid owners call no runtime factory;
invalid/denied slots retain weights but call no backend factory, SP/policy/store,
backend dispatch or dial. Context is a consistency/diagnostic projection only.
Malformed HTTP descriptors, including missing/out-of-range declared Service
ports, deliberately return 500 per both vendored v1.6.2 invalid-share contracts;
valid Service live section/endpoint failures retain 503. The old
`invalid_backend_with_service_key_still_captures` test retains its identity,
malicious schema/context and installed policy, but its unsafe capture expectation
is explicitly superseded by None and a zero-query spy. Preserve AI 503, gRPC
terminal semantics, zero-weight sticky/named targets, first absent policy capture,
retry Arcs, TLS/appProtocol and SSRF. AI/Canary/DIU changes are mechanical identity
consumers only. L4, coherent standard-Service ExternalName, ExtensionRef/mirror and
plugin outbound migration remain separate batches. Additional paths or material
deviations need reviewed amendments before editing. This is plan approval only.

R2a narrow fixture amendment received independent Astra scoped PASS: DIU's seven
and Canary's four successful Service-jump fixtures require port 80 instead of
the old absent port. Preserve their identities and other inputs/assertions;
each module adds an explicit missing-port case with matching valid identity,
compiled Invalid, zero jump and the existing Reject 403. Production
classification is unchanged. Root authorized this within the frozen 51 paths.

Focused HTTP verification first passed 481 cases. Gateway doctest archive
creation then failed with ENOSPC, not a Rust or assertion error; preserve the
failed log. Root coordinates recovery of obsolete incremental build cache only
in this workspace with Cargo stopped, preserving sources, dependency artifacts,
test binaries, logs and baseline/review snapshots. Final verification and result
review remain pending; neither focused success nor infrastructure recovery is
final acceptance. Recovery removed 648 workspace incremental directories whose
entire contents predated 2026-10-01; available space rose to 77 GiB. The cache
manifest is `/tmp/edgion-ref-r2a-stale-incremental-cleanup.json`. No source,
dependency archive, current test binary or other workspace was deleted.

Read-only Astra R2b candidate inventory found that TCP/UDP/TLS units already hold
actual Route Arcs, but selectors clone raw references and four dial paths still
classify, format keys and parse ports. Both TLS paths retry malformed declared
ports, transferring their invalid share to other slots. The next independently
reviewed batch should reuse cold canonical Service targets and original index
selectors, make Invalid/Denied terminal before discovery, and preserve valid
endpoint/connect retries. Keep existing live L4 degradation, no new policy/TLS
capture or TCP/UDP retries, and UDP's existing pinned address/session lifecycle.
Fourteen production Gateway paths are candidates, not an approved allowlist;
mechanical mirrored fixtures may be needed if source construction returns Result.
Revalidate after R2a commit. Standard Service ExternalName remains R2c.

R2a final acceptance: `/root/native_route_astra_result` returned independent
Astra scoped PASS after reviewing all actual changes and surrounding authority,
generation, weight, SP, DirectEndpoint, policy/TLS and error paths. Owning backend
knowledge was corrected; the sole newly exposed test Clippy warning was fixed
without changing inputs/assertions and the complete AI HTTP module passed again.
The final 51-path scope SHA-256 is
`5905b27f2792ced537858bfdfed2b26d3efcce28df2a3393feca9b5dd0171e58`;
patch SHA-256 is
`5774655c3051fd05f57bc259be081e2c6d4db6a139b89a7436ccbf16eb8816bc`.
Root and reviewer independently reproduced every baseline/final file hash and
the exact manifest digest under `/tmp/edgion-ref-r2a-final`. There are 49 changed
implementation files plus this ledger; 347 original test identities remain,
11 were added and ignored attributes are unchanged.

Fresh focused evidence: Resources refs 71; Resources docs 22 with ten existing
ignored; Gateway HTTP 482, backends 275, GRPC 68, AI HTTP/dispatch/model 16/30/4,
DirectEndpoint 11, DIU 10, Canary 14; Gateway docs eight with seven existing
ignored. Counts overlap, not cumulative. The gRPC error mapping test uses an
in-memory H1 wrapper, not a full H2 conformance exchange. Final workspace
check-r2 and Clippy-r2 pass without warnings in 18.47s and 20.05s. Formatting,
docs, SSA, metrics, Pingora, transform/reference drift, layout (1000), tracing,
English and diff guards pass. Logs use `/tmp/edgion-ref-r2a-`; retain earlier
compile, ENOSPC and Clippy-warning logs. No schema/CRD or generator inputs changed;
C0's unchanged CRD/plugin/generator evidence is reused, not a new R2a run.
This is R2a acceptance only, not L4, Service.type/DNS, later consumer migration,
full workspace tests or live conformance. Stage only verified scope and commit
locally; record the successful hash in the next batch. Never push.

R2b concrete ten-point plan and clarifications received independent Astra scoped
PASS from `/root/checkpoint_astra_audit` at d37b890a8. Root froze 56 paths
(19 production, 21 tests, 16 owning documentation) and authorized delegated
execution. Immutable originals, manifest and 185 source test identities are in
`/tmp/edgion-ref-r2b-before-d37b890a8`; the unchanged existing R2a test binary
listed 5110 full Gateway test identities without running test bodies or Cargo.
Preserve both source and compiled identities and ignored attributes. Additional
paths or material design/fixture changes need renewed approval before editing.

The approved L4 runtime compiles actual TCP/UDP/TLS first-rule loans into private
cold targets and immutable original-index slots, with borrowed sealed selection.
Private unit getters retain actual Route Arcs and derived generation pairing.
The real TLS one-attempt helper owns one selection from the unit and the actual
resolver-entry closure. Invalid/Denied is terminal and makes zero resolver calls;
valid live endpoint/connect failures retain existing retry limits and delays.
Independent final review must inspect both real Terminal-return branches; unit
attempt tests are not copied-loop, connector or wire evidence. Existing route-stage
plugins retain their ordering for valid owners; there is no L4 backend plugin
factory or backend policy capture to claim as tested.

Shared partial update removes old same-key add entries before initialization.
Full-set publication follows successful compilation; partial publication removes
all touched raw keys then inserts final successful cached units, including phase
overlap winners. Tests cover invalid add/update/remove/repair, former listener
withdrawal and local publisher material demand with positive controls. Omitted
namespace preserves the bare raw manager/publication/removal key while canonical
Service targets use default namespace. Old selected Arcs and UDP address pins
retain their existing lifecycle. No added L4 policy/upstream TLS, TCP/UDP retry,
UDP revocation, Controller/Resources/publisher/WRR, R2c or parent/filter migration
is authorized. Root owns this ledger, workspace gates and commit; the fresh Astra
result reviewer remains `/root/native_route_astra_result`. This is plan approval
only, not implementation acceptance or live conformance.

Read-only R6 inventory at 0fea0a618 confirmed a DNS producer parity defect:
the ordinary Service handler strips denied cross-namespace resolver annotations,
but the background DNS service lists raw API Services, reads LinkSys and resolves
DNS without that Grant gate before publishing EndpointSlices. Prioritize a
separate actual-Service annotation/source-authority batch, including freshness
around awaited GET/DNS and each family write. Denied selection falls back using
the existing resolver rules before any denied LinkSys GET/query; a permitted but
missing/wrong-type LinkSys retains its distinct System fallback. Reuse canonical
HTTPDNS/TLS preparation and existing failed-generation policy; add no handler IO.
The producer's skip fingerprint also lacks observed Service owner UID, allowing
same-key recreation with identical content and an old-UID Slice to skip its owner
repair. Bind generated association and observation to the actual Service.

EndpointSlice Gateway discovery already rebuilds old and new associations
atomically. Controller only wakes the incoming owner, but no old-Slice-derived
production reader was found: report this as narrower wakeup/lifecycle evidence,
not a confirmed stale Gateway pool. Preserve distinct malformed-Slice policies.
Dedicated Service Rust/Python naming and full process namespace/name scope are
already correct; no name-only downgrade was found. Later annotation, instance,
logging/global-plugin/fixed-string batches must retain their distinct grammars,
fail-close sentinels, cold construction and provenance. ACME generated outputs
remain operation-specific R8 work, with strict existing reference groups.
Inventory is not fresh runtime evidence or plan approval; R2/R3 overlaps require
revalidation before any mutation.

Read-only R7 inventory confirms Controller managed HTTP TLS/template binding is
already implemented. Gateway ForwardAuth/Guardrail/AiGuard still use detached
construction, losing owner namespace and ignoring Service group; migrate those
actual entries without duplicating Webhook's owning construction. Preserve the
spec's custom-Service policy: no new Service Grant/dependency/forwarding-policy
ancestry; TLS material remains separately grant-gated. Service TLS enablement
must honor its documented no-scheme rule while explicit HTTPS URLs remain secure.
The internal FS/etcd HTTPRoute Edgion auth branch additionally needs R3 declaring
filter provenance, material binding and root wakeup, including delegation; this
does not enable native ExternalAuth. Mirror runtime target binding also belongs
to R3. CredentialSource already has extensive source/consumer/Grant/Lease/CAS and
cleanup fences. Remaining candidates are actual OAuth transport capture,
operation-bound generated outputs and controlled issuer/publication race evidence,
not a wholesale lifecycle rewrite or new revocation policy.

Read-only R8 inventory identified native Gateway TLS pre-validation dependency
creation and parse/status namespace-default differences; EdgionTls validation
also reads Grant/material before parse registers recovery edges. Prioritize
actual field source/slot binding and invalid zero-edge/read gates while retaining
existing TLS modes, precedence and eligible generation retention. BTP local CA
status currently keys by name only; bind kind/scope and independently verify its
standard partial-valid-CA policy. EBTP health needs actual active policy fields
and coherent local material pairing, reusing its complete-config generation/poll
fences. ACME already centralizes strict group/kind and has real attempt/Lease/
source/UID/RV publication guards; add owning read/create/patch/output capabilities
without introducing EAB Grant policy or altering separate output namespaces.
R5 policy-owner adapters and R2 selected-TLS seams must be reused/revalidated.
Original EdgionTls dependency-order and absolute TLS knowledge claims need
same-batch reconciliation. These are source inventories only, with no approved
mutation, fresh tests or conformance verdict.

R1 implementation evidence so far: Resources references 70 passed; doctests
22 passed with ten existing ignored cases; Controller production library check
passed without warnings. Controller test compilation is still being corrected;
initial helper r1/r2 failures remain in separate logs. This is not final batch
acceptance. The first allowlist amendment adds the existing private HTTP test
support fixture path (33 total): an actual Route with a valid fixed name,
verbatim namespace and unchanged cloned rule feeds the new owning admission
function. Independent review first passed with unknown runtime, then the original
confirmed Astra plan reviewer independently returned scoped PASS before edits.
No production compatibility API or visibility expansion is approved. The original
182 test identities and the support baseline are retained under
`/tmp/edgion-ref-native-before-8dab81bc`.

The full handler first run passed 648 and failed four tests. Independent Astra
amendment 2 approval permits explicit empty group in two existing ExtensionRef
negative fixtures, a Tokio runtime for the new gRPC status test, and an exact
canonical error assertion in the existing BackendTLSPolicy mirror. That mirror
is the sole additional path (34 total); policy and ExtensionRef production code
remain outside the batch. Preserve the failed run and all original assertions
except the stale error literal; fresh corrected suite results remain pending.

Corrected handler results passed 652/652 and the broader resource-processor
subset passed 909/909. Gateway weight tests passed eleven and direct denial
passed one. The additional named-jump guard exposed a pre-existing helper whose
Service port was absent; canonical classification was unchanged from 8dab81bc6.
Independent Astra amendment 3 permits the sole Gateway test mirror as path 35:
use a valid port 80, preserve original assertions, and assert both relevant slots
compile as Service. Rerun its entire module and existing guards before acceptance.
No Gateway production change is authorized. The owning
`denied-named-jump-fails-closed.md` still describes pseudo kinds as valid Service
targets; reconcile it in the separately reviewed R2/F1 documentation scope.

R1 final acceptance: `/root/native_route_astra_result` independently reviewed the
actual diff, original rules and knowledge, source/child authority, root wakeups,
dependency ordering, coalesced replacement and invalid/denied lookup boundaries.
It returned scoped PASS after the sole introduced `manual_let_else` warning was
corrected. The final 35-path manifest matches scope SHA-256
`82ea5f599c026ca6a1201d395205f6a69ddb655ba2c746347f087a69e7be648d`;
29 implementation paths changed, plus this coordinator-owned ledger. All 294
original test identities remain and 16 were added.

Fresh focused evidence: Resources references 70; doctests 22 with ten existing
ignored; Controller owner helper six; handlers 652; broader resource processor
909 (includes handlers); Gateway peers 98; weighted selector eleven; gRPC routes
66; selected-Service context three. Counts overlap and are not cumulative.
Failed initial fixture/compile logs remain available. Final check-r2 and
clippy-r2 pass without warnings in 27.18s and 42.75s respectively. Formatting,
docs, SSA, metrics, Pingora source, transform and reference contract drift,
unit-test layout (999), Gateway tracing, English and diff guards pass. Unchanged
CRD/plugin-condition and generator inputs retain C0's 28/eight and three-test
evidence; these were not new R1 runs. Logs use `/tmp/edgion-ref-native-`.
This scoped acceptance does not establish whole-manifest or live conformance
completion. R2 plan review and implementation begin only after R1 is committed.

R2b final acceptance: `/root/native_route_astra_result` independently reviewed
all actual production/test/knowledge changes, original rules, canonical owner
loans, weighted slots, actual resolver boundaries, both TLS terminal/retry loops
and invalid replacement/publication cleanup. It returned scoped PASS. The final
56-path manifest has 55 changed paths plus this coordinator ledger; all 185
original scoped source IDs and 5110 compiled Gateway IDs remain (195/5120 now),
with unchanged ignored attributes. Scope SHA-256 is
`b5f5dd8c2f11e911b0be2419d7f6f4f706acce607c050d580cd3d7eb846a0c16`;
patch `732c1935b5e29c694fa5b505503933fee58895ea21ec8cc65b687514fcc2cb87`
and manifest `020c01dd35c082ceb1da963091ca6564a46ad510c045b1511006e4d4c4d5925b`.
Both root and reviewer independently reproduced the final hashes. Artifacts and
superseded guard-failure snapshots are preserved under `/tmp/edgion-ref-r2b-`.

Fresh focus passed runtime 12, TCP 24, UDP 51, TLS 83, common 37, delta two,
bridge six, backends 278, selector 25, and Gateway docs eleven with seven existing
ignored. Counts overlap. EOF whitespace and four test-only source accessors were
corrected within scope after the layout correction received independent Astra
plan PASS; fresh affected tests and layout passed. Final workspace check-r2
passed in 14.85s, Clippy-r1 in 1m04s without warnings. Formatting, docs (1447), SSA,
metrics (103), Pingora source, transform/reference contract drift, unit layout
(1001), Gateway tracing, English (including scoped task records) and diff guards
pass. No schema, CRD, generator or dependency inputs changed; their earlier C0
checks were not rerun as R2b evidence. This does not establish live connector,
wire-conformance, full-suite, whole-manifest or TLS Terminate product support.
R2c plan review and implementation begin only after the R2b commit succeeds.

R2c concrete ten-point plan received independent Astra scoped PASS from
`/root/native_route_astra_result` against 5cf413bbc, including the explicit
borrowed resolver callback and None-namespace controls. The exact 31-path scope
is three production, three test, seven fixture and eighteen documentation files.
Root froze thirty existing files and one absent new mirror under
`/tmp/edgion-ref-r2c-before-5cf413bbc`; manifest SHA-256 is
`d8dd7a5f61392ca4f501d69bfb8f91d5b0a3c8c5775e83679b63d7c3a0e13f7a`.
The baseline contains fifteen existing scoped source test identities, all 5120
compiled Gateway identities from the accepted R2b binary, unchanged ignore
attributes and exact hashes for seven fixtures. Only ten native pseudo-kind
values may become `Service`; names, ports, security settings, assertions and
scenario transitions remain unchanged.

R2c amendment 01 received independent Astra plan scoped PASS: add only
`edgion-gateway/tests/unit/backends/validation_test.rs` as path 32, importing
`ServiceSectionKey` from the existing canonical `backends::discovery` export.
All three test bodies, identities, inputs and assertions stay unchanged; no
production compatibility export or visibility change is permitted. The original
31-path baseline remains immutable. The separate amendment baseline is
`/tmp/edgion-ref-r2c-before-5cf413bbc/amendment-01/manifest.json`, SHA-256
`fa434cb9e8855656282a3ac983eb78e352d226961ee78072cfdbcc91a90c5147`;
the original file SHA-256 is
`924ede02b7aa7ff5c2c1e837af537eac5d8f2f749bff1b5b892e0fd19e337673`.
The first focused compile failure is retained in
`/tmp/edgion-ref-r2c-native-r1.log`: this import and the new mirror's private
gRPC-module import failed compilation. The new mirror uses existing public
re-exports within its approved path. Root's interrupted turn delayed the internal
execution message; execution is now authorized, and focused verification resumes.
These static/preparation checks are not final implementation acceptance.

Recovery evidence is stored persistently under
`tmp/resource-reference-contract/r2c-recovery-20261008`. Root reconstructed the
original sources from immutable 5cf413bbc and reproduced the original 31-path
manifest bytes and SHA-256 exactly. The separate path-32 source hash also matches
its recorded baseline; 18 scoped original source test identities and ignore
attributes are recovered. This is explicitly reconstructed source evidence,
not a claim that the original temporary directory survived. Earlier raw compile
failure logs and the 5120-entry compiled identity listing are unavailable. Fresh
verification logs and a baseline/current identity control are required; do not
fabricate those old files or treat their recorded outcomes as fresh acceptance.

Fresh persistent focused reruns pass: native dispatch 12, backends 290,
upstream-peer controls 101, gRPC 68, DirectEndpoint 16, SP 22 and Gateway doctests
11 with seven existing ignored. Logs are under
`tmp/resource-reference-contract/r2c-verification-20261008-r1`; these suites
overlap and their counts must not be summed. Canonical binary preparation is
rerunning for fresh provenance; runtime traffic and final result reviews remain
pending.

The deep Astra reviewer independently approved a narrowly scoped direct
canonical shared-topology verification entry. The top-level runner requires
unrelated macOS loopback aliases, while these DNS fixtures use only 127.0.0.1.
Use the existing start_all_with_conf entry once with both DNS suites, existing
runner helper gates and the actual static four plus all eight dynamic wait-phase
transitions on that same stack. No script, assertion or system alias is changed;
this is not a full-runner or conformance PASS. Execution requires fresh
resource_diff (startup otherwise skips it), one-Gateway/unknown-field checks,
zero preexisting matching processes and free required ports. Protect and restore
the original token, .current marker and shared certificate directories privately,
including absent-path state; canonical generators can rewrite existing material.
Startup failure must locate its owned new work directory before .current is
published. Cleanup terminates only owned verified PID/executable pairs and
restores original global state on every exit. Root authorized execution after
that independent plan PASS; the final actual-result gates remain separate.

R2c implementation is frozen under
`tmp/resource-reference-contract/r2c-final-20261008-r1`. Root independently
reproduced the exact 32-path digest
`92cc5566f8942b74ee5e6d5b4c4974bffeee6f9fc66682f4b19387bcdeee823c`,
manifest digest
`974ea6f6c6a30d0946ad7e95246a6e1b054ba6f6d4f1dda8de18004343ed1ad3`
and patch digest
`40de4b136cbd0a18082ba7b2595c481c4f9cef2344b51b51f5e091d638e733ce`.
All 32 scoped files changed within the effective approved allowlist. Root's
progress ledger is separate; independently owned review-results stay untouched.

Fresh baseline compilation in an owned clean detached worktree at immutable
5cf413bbc recovered all 5120 compiled Gateway test identities. Current compilation
lists 5132: no old identity was removed and exactly twelve native dispatch tests
were added. The comparison digest is
`d7953d12b377e54b7ad2e13eb05da5fb083e8ac7be6cebb5e7ecc444961c1b31`.
These list-only invocations did not execute the full unit suite. The owned
baseline worktree was removed successfully; unrelated worktrees were untouched.

The direct canonical live run passed four static cases and the complete dynamic
scenario on one owned stack, `testing_20261008_084440`. Its unchanged implementation
awaits eight fallible phases sequentially; exit zero proves the sequence returned
successfully, while raw stdout intentionally reports only scenario PASS. Exact
phase-source hashes and this inference are recorded, without invented per-phase
stdout. Root verified resource sync, single-Gateway and unknown-field evidence,
owned PID cleanup and exact restoration. No matching runtime process remains;
original credential backup copies were deleted after restoration.

`/root/r2c_fresh_result_astra` independently reviewed all frozen paths, surrounding
calls, original authority, source/compiled identities, fixtures, live provenance
and restoration, and returned explicit scoped PASS with no blockers. The second
deep Astra actual-result round is active. Fresh root fmt, docs, SSA, metrics,
Pingora, transform/reference drift, layout (1002), tracing, English and diff guards
passed. Workspace all-targets check passed in 1m 36s without warnings. Clippy is
running at that checkpoint; no commit or whole-task acceptance was claimed.

The second independent deep result reviewer, `/root/r2c_deep_result_astra`,
subsequently returned explicit scoped PASS with no blockers after tracing
surrounding retry/SP, policy absence, discovery activation/retirement, canonical
DNS and TLS and surviving L4/mirror selectors. It independently reproduced all
scope, identity, fixture, runtime provenance and restoration evidence. Workspace
Clippy passed in 1m 22s without warnings. All required scoped gates now pass.
Root scanned the applicable graded MUST rules against the final diff: canonical
DNS/TLS/selection reuse, no added hot-path allocations/locks/tracing/metrics,
strict target/failure semantics, English-only and mirrored tests remain intact;
schema/secret transport, body storage and Controller queue rules are unchanged.
The original issue remains open for R3 through R8 and whole-task closure. Stage
only the approved 32 files plus this progress ledger and commit locally; never
push or include independently owned review-results.

R2c commit succeeded: `f65b863c7cda6ec9936569d91e7bd8897824c4b2`,
`Select native Service backends from one captured generation`. Exactly the
32 approved files plus this ledger were committed; the original issue and
independent review-results were excluded. Post-commit status contained only
unrelated untracked review-results. No push occurred.

R3a planning starts from that clean checkpoint and is delegated plan-only.
Revalidate ExtensionRef source, Controller dependencies/validation and Gateway
publication/request consumers before freezing scope. Preserve the approved
existing delegation transport: authored Controller loans use the real child
identity/slot before cloning; Gateway effective target-only loans use the actual
published root/field and controller-owned child-local resolved_namespace.
Spec section 5 limits UID/generation/revision checks to existing binding protocols;
do not invent a new provenance carrier or claim the Gateway receives an original
child UID it does not carry. Effective views cannot mint Grant authority.
Separate mirror, descriptor and later credential/parent/DNS/TLS consumers remain
tracked. Any split needs coherent intermediate behavior, original test retention
and independently reviewed exact scope before editing.

The serial implementer is authorized to reuse one private attempt-scoped Service
projection/section/membership-verified runtime for type dispatch, endpoint
readiness, SP validation and LB. Exact actual `Service.spec.type=ExternalName`
selects canonical DNS, retaining hostname/annotations/checked port/appProtocol
through the await. Ordinary native selection removes redundant reloads without
adding allocation or a new store. Live health/outlier and captured EBTP/TLS keep
their separate existing lifetimes; each later peer invocation captures current
geometry. Invalid/denied inputs never enter the capture/query seam. Old native
raw wrappers are removed after their final production callers migrate, preserving
all tests through actual-owner fixtures. No standard aliases, new schema/mode
carrier, R6 annotation authority, R7 custom grammar or L4 async DNS is added.
Focused tests, actual shared-stack DNS/flap scenarios, fresh independent result
review, workspace gates and commit remain pending. This plan is not acceptance.

## Serial batch queue

The eight remaining implementation batches are provisional boundaries. Before
each batch, inspect current code and freeze exact files, manifest rows, tests and
acceptance criteria. Split a batch when independent ownership or complexity
requires it; do not silently expand an approved allowlist.

| Batch | Required result and critical acceptance | State | Commit |
| --- | --- | --- | --- |
| C0: current checkpoint | Independent accumulated-diff audit, resolve immediate introduced blockers, run required pre-commit gates, stage only verified task changes; describe partial completion honestly | complete; scoped final PASS and all gates passed | user snapshot 0f40e2708; closing 8dab81bc6 |
| R1: native Route backends and delegation | Actual five-Route owners; authored terminal loans alone mint Grant; invalid descriptors cause no lookup/edge; child authorizes delegated targets, root owns wakeup; dependencies precede Grant reads; preserve slots/weights, AI facts and coalesced recovery | complete; fresh independent Astra scoped PASS and all gates passed | 0fea0a618 |
| R2: final Gateway backend selection | Consume cold canonical targets through HTTP/GRPC/L4 final dial; retain weighted invalid slots and denial behavior; no new request allocations, locks or duplicate classifier; preserve direct endpoint/retry semantics | complete; R2c two fresh Astra rounds, identity/live proof and all gates passed | R2a d37b890a8; R2b 5cf413bbc; R2c f65b863c7 |
| R3: filters, ExtensionRef and mirror | Actual HTTP/GRPC filter owners and child namespaces; authored/resolved coverage; correct runtime compilation, local descriptor ownership and recovery; mirror denial affects only mirror with role-qualified dependencies | R3a1/R3a2 complete; R3b exact 31-path plan PASS, baseline frozen, implementation authorized; descriptor portion pending | R3a1 05e7debf5; R3a2 75a5363f5 |
| R4: generic dependency and status | Replace remaining raw plugin credential/ConfigMap dependency and status paths with actual-owner authority; exact generation/disabled/delete/recreate handling; readonly status agrees with acquisition and runtime | pending | pending |
| R5: attachment and policy references | parentRefs, allowedRoutes, GatewayClass parameters, GatewayRouteIndex and policy targets use appropriate object/type/selector identities; preserve namespace selectors, consent, controller ownership and ACME strictness | pending | pending |
| R6: annotations and fixed identities | Preserve per-field annotation parsing; bind DNS LinkSys and fixed output references to actual owners; retain separate malformed EndpointSlice fallbacks; verify dedicated Service producer parity and process Gateway scope | pending | pending |
| R7: plugin outbound calls and CredentialSource | ForwardAuth/Guard/AiGuard/internal Edgion auth use actual Service namespace and local TLS through canonical HTTP transport; OAuth bootstrap retains actual capture; grants/leader/lease/expiry checked across awaits; credentials remain redacted | pending | pending |
| R8: Gateway/backend/health TLS | Native strict Gateway refs, local BTP CA/client and finite custom health aliases; coherent whole material pairs and private effective profiles; correct fallback, no plaintext widening, rotation/pool/cancellation/recreation behavior | pending | pending |
| F1: migration cleanup and documentation | Lookup-site audit against every manifest row; remove obsolete raw APIs and detached constructors; resolve introduced lint and hot-path regressions; reconcile spec/default tables, schemas, source contracts, original knowledge and user docs | pending | pending |
| F2: whole-task verification and closure | Repeated fresh Astra full-result rounds emphasizing Grant, coverage and conformance; required workspace checks, meaningful dynamic tests and fresh admission/conformance evidence; resolve blockers, remove selected issue and stale indexes only after acceptance | pending | pending |

## Commit gate for every batch

1. Explicit approved plan and frozen scope; independent fresh result PASS.
2. Focused meaningful tests and required project formatting/build/lint/docs/SSA/
   metrics/Pingora/transform/unit-layout gates. Check CRD coverage, reference
   contract drift, English and sensitive-data/observability rules when applicable.
3. Inspect staged diff against current HEAD and design-rule MUST rows. Do not
   silently include unrelated changes or erase the original issue before closure.
4. Commit with an English message, verify the hash and remaining working tree,
   update this ledger in the next authorized batch. Never push.

## Final closure checklist

F2 tooling inventory at 5cf413bbc is read-only, not acceptance. Docker/arm64,
kind and kubectl are available, as is a cached Go 1.26.8 toolchain for the pinned
Gateway API/conformance v1.6.2 module. Shared OrbStack and the existing Standard
kind cluster contain historical artifacts and must remain untouched. Use a new
disposable cluster, explicit isolated KUBECONFIG/context and current-revision
images; preserve exact image digests, CRD bundle/channel/SHA/served GVK, features,
Grant settings, raw logs/reports and cleanup evidence. Canonical entry points
are `edgion-tests/compatible/run_compatible_test.sh`,
`edgion-tests/k8s/scripts/run_k8s_integration.sh`, and the conformance setup and
in-cluster runner. Revalidate their source and environment at execution time.

Conformance setup defaults to Experimental; Standard needs an explicit pinned
Standard install and `--skip-crd`. Clean-slate/cleanup mutates fixed namespaces
and components, so never point it at shared or existing clusters. The current
runner supports arm64 but original knowledge still describes fixed amd64.
CRD-mismatch allowance and fixed report names require independent actual-channel
evidence and unique archived results. No full reference live-admission runner
was found: OpenAPI offline checks and ConditionSet admission tests do not prove
reference CEL/defaulting/pruning. Native `group: core` may be admitted as a
non-core group; unsupported runtime behavior, not universal admission rejection,
is the contract. Preserve declared features/skips and historical Standard
failures rather than substitute Experimental PASS or narrow the profile.

- Every finite manifest row has an actual-owner adapter and complete consumer,
  authorization, dependency, status and final-consumption evidence or an explicit
  approved exclusion. Inventory alone is not coverage.
- Defaults distinguish omission, empty, literal core, custom group, required kind
  and namespace; standard Gateway API admission remains strict and vendor CRDs
  stay unchanged. Custom conveniences stay within the approved finite manifest.
- Old raw authorization/lookup/transport constructors are removed after their
  last migration. Shared mechanisms have one canonical implementation.
- Invalid references perform no fabricated lookup or dependency; valid missing
  and denied references preserve recovery. Async authority is fenced on both
  sides of awaits; rotation and delete/recreate remain correct.
- No added request-path allocation, blocking lock, secret exposure, request-level
  tracing or duplicate TLS/HTTP implementation survives final review.
- Fresh conformance records identify runtime revision/artifact, suite version,
  actual installed CRD channel/GVK, grant switch, feature set, fixtures, skips and
  raw reports. Targeted admission and full-profile runs are reported separately;
  historical failures are not silently relabeled as new PASS.
- Run final workspace gates and repeated independent Astra rounds after the last
  relevant code change. Do not substitute provider acceptance for whole-task PASS.
- Resolve knowledge conflicts and stale inbound links. Remove the selected
  original issue only after complete acceptance; trim/archive the implementation
  task according to the project lifecycle without deleting unrelated tasks.
- Commit final cleanup locally and report commit hashes, verified behavior and
  any explicit remaining limits. Never push.

## R3a1 approved implementation gate

At immutable HEAD f65b863c7, the actual-owner ExtensionRef migration is split
serially into Resources/Controller (R3a1) and Gateway (R3a2), with a separate
review, verification and local commit for each. Confirmed gpt-6-astra
`/root/r2c_deep_result_astra` independently reviewed the ten-point plan and all
23 existing paths and returned SCOPED PLAN PASS. This does not accept Gateway
preflight closure or whole-task conformance.

The private owning field loans distinguish authored references from effective
resolved-rule stamps, retain actual child identity and field positions, and mint
no Grant authority. Register all valid child dependencies before later drops;
read targets only for structurally eligible existing scope checks, from the raw
desired generation. Clear plugin dependencies once at parse entry and extend
that set in subsequent root/status registration. Keep raw-None root wake keys
separate from canonical default target namespaces. Invalid owner, field or stamp
must produce zero dependency edges and target reads. Missing target repair,
dropped-child recovery and coalesced replacement require meaningful coverage.

The source baseline is retained under the ignored
`tmp/resource-reference-contract/r3a1-baseline-20261008/`: 23 immutable files,
188 attributed test functions, ignore attributes, original hashes and exact
allowlist. Baseline manifest SHA is
`b87bb3ec17dab744857913a69095f4dc725dab138de16df950725a8cd02a78ea`.
Fresh baseline compiled listings completed before edits: Resources 1922
and Controller 1731 tests, both command exits zero. Root explicitly
authorized execution after both controls completed. Existing schema, transport, mirror, native ExternalAuth support and
ReferenceGrant policy remain outside R3a1. Fresh final review, second deep Astra
review, targeted verification, workspace gates and local commit are pending.

A later continuation again found no surviving prior subagent threads. Source
and all R3a1 persistent artifacts remained intact; there was no active owned
Cargo process. New explicitly configured gpt-6-astra roles resume the same
approved scope: `r3a1_impl_resume` is the sole implementer, `r3a1_fresh_astra`
reserves the fresh first final review, and `r3a1_deep_astra` reserves amendment
and second deep final review. No plan or acceptance is fabricated from the
thread recovery. The first new Resources test compile failed on a Result
equality assertion requiring an absent production PartialEq derive; the test
was corrected to pattern matching without changing production schema. The
retained second raw log reports 74 focused reference tests passed. Controller
lifecycle coverage and owning documentation remain in progress.

Execution recovery observed an extra task-local Controller test process and
additional in-scope edits while the resumed implementer was active. The resumed
implementer paused, root preserved all 23 scoped files plus its own ledger under
`tmp/resource-reference-contract/r3a1-ownership-recovery-20261008-r1`, and the
checkpoint remained byte-identical for 98 seconds. The extra task process ended;
other workspace processes were verified unrelated and left untouched. No
surviving old agent was manageable. With no renewed writes or extra task process
observed, root resumed one implementer under existing user authority and retained
continuous scope monitoring. This is an observed resolution, not user confirmation
or a final acceptance claim.

Controller handler attempt 1 failed because an existing test-support helper lost
an implicitly imported pure validator; a cfg(test) import restored that fixture
within the approved source path. Attempt 2 passed 660 cases and failed one new
lifecycle test that assumed root-before-child queue order. The new assertion was
changed to require the root wakeup under a bounded wait without imposing queue
order; its dependency, status, recovery and replacement assertions remain. Both
raw failure logs remain in the R3a1 verification directory. Reverification is
pending.

A narrow independent Astra adjudication found invalid-source-only status
dependency withdrawal to be a requirement-preserving refinement: valid status
remains additive, normal generation replacement clears at parse entry, and an
invalid source has no valid child set to erase. Standalone invalid status may
clear stale edges and withdraw derived fields, but must create no edges or reads.
This is scope precision only, not final result approval.

R3a1 candidate r1 passed workspace check and Clippy (exit zero, 264.08 and
410.983 seconds) after all focused checks. The immutable 23-path result is
retained in `tmp/resource-reference-contract/r3a1-final-20261008-r1`; its scope,
manifest and patch SHA256 values are respectively
`6550fcbc70097c7a4ac88a1583315407a29dac3d508c4be023c5c8d1449e3eae`,
`2322bb1eecde0afb717cd1c9752e5d8b0730ef30811128cc6e8fa8b60873c471`, and
`6498f11a9a0b8ce1d7f400673b420a4b73f2b836c2459ce333f0956179963081`.
Gateway compiled identity control preserves all 5132 tests with no additions or
removals. These are r1 evidence, not acceptance of later corrected bytes.

The fresh first Astra result review returned SCOPED BLOCK for R3a1-F1: an empty
or slash-containing ExtensionRef name with supported group/kind is safely
rejected but HTTP status falsely claims the group/kind is unsupported. The
separate deep Astra reviewer approved the narrow correction plan: distinguish
the name error using the existing group/kind-only helper inside the canonical
validator error branch, preserve unsupported-group/kind wording and priorities,
and prove direct/delegated status and zero dependency edges. After both root
Cargo gates ended, root explicitly authorized the sole implementer to execute
within the same 23 paths. Replacement evidence and two result-review passes are
required before local commit. R1 artifacts remain immutable; the original issue,
Gateway R3a2 and subsequent batches remain open.

The correction passed Controller `resource_processor` verification: 921 passed,
zero failed/ignored, 822 filtered (r2 `controller-processor-r4`, exit zero).
Three new fixture attempts remain recorded: missing Gateway masked the intended
reason, a borrowed runtime was mistaken for an Arc, and a plain test lacked the
Tokio reactor. The final actual RawStore Gateway/owning Arc/async fixture retains
all assertions and test identity. Source identities are 188 -> 203 and compiled
Resources 1922 -> 1925 / Controller 1731 -> 1743, with no old removals or ignore
changes. The owning guard now distinguishes existing Gateway union/filter-list
construction gates from still-pending independent name/owner consumption.

Corrected immutable candidate r2 is
`tmp/resource-reference-contract/r3a1-final-20261008-r2`, with scope SHA256
`2d820cfc7da6bdb42881ac5feb1da2a8e293b05a833097b336af7d0376ab8aca`,
manifest `bbd9ea9ed9f6175aad54d36e4a54e73a5b73953626be1c8d4fc0fc31397290ea`
and patch `6764e45d99bcf20e9cf8d720f2b8bb42c4bd7df646cd18df9ba89b99f5665b8b`.
Only six previously approved paths changed between r1 and r2. All r2 root gates
passed: workspace check 24.188 seconds, Clippy 34.100 seconds; format, agent docs,
SSA, metrics, Pingora, transform/reference drift, unit layout, gateway tracing,
English and diff checks. Raw commands/results remain in the r2 verification
directory. Resources/Gateway focused r1 results apply to unchanged relevant
source; they are not represented as fresh r2 runs. The fresh first Astra reviewer
is reviewing this corrected frozen result; the second deep result and local
commit remain pending. No whole-task or conformance acceptance is claimed.

Both explicitly configured Astra reviewers independently returned SCOPED PASS
for frozen r2: fresh `r3a1_fresh_astra`, then deep `r3a1_deep_astra`. F1 is
resolved and neither found another correctness blocker. Root's raw-log inspection
found two introduced test-helper `question_mark` warnings in both r1 and r2
Clippy output. The commands exited zero, but these bytes are not warning-free.
The deep verdict explicitly retains that qualification. This does not change
the earlier successful command exits or invent clean lint evidence.

A separate narrow cleanup plan received independent Astra PASS before edits:
replace the two new Option-returning spy helpers' equivalent let-Some-else
early returns with `?`, preserving every assertion and identity, with no
production or scope change. Root authorized the sole implementer after the
second frozen r2 review ended. Fresh affected verification, warning-free root
gates, replacement r3 hashes and both reviewers' renewed byte verification
remain required before commit. Readonly R3a2 inventory identified the actual
Gateway consumers but is not an approved or executed next plan.

Final immutable candidate r3 is
`tmp/resource-reference-contract/r3a1-final-20261008-r3`: scope SHA256
`9d2585bb80aaf2130d8a247e870dc3f4ee9a09c28995663943cd58a4ea99074c`,
manifest `64aef09f902a7f5b10d7aa146bc9389e5b19a1be687822e72a7a918ea800d0f4`,
patch `375bf39681a45df981035fd2dbe7e0cc353254db306411b9268329417c8aa79b`.
Both independent Astra result reviewers renewed SCOPED PASS on these final
bytes. Each reproduced file/patch hashes, the old identity/ignore controls and
equivalent helper simplifications; neither found a blocking finding. Root also
matched the actual frozen result. Fresh Controller verification passed 921,
with zero failed/ignored; unchanged identity totals remain 188 -> 203,
Resources 1922 -> 1925 and Controller 1731 -> 1743.

Final r3 workspace check and Clippy passed in 18.022 and 23.198 seconds. Root
inspected both raw logs and confirmed zero warning/error diagnostics, rather
than inferring that from the exit codes. All eleven recorded format/guard
checks passed. Root's graded MUST scan found no new secret/schema/transport
field, logging surface, request allocation, handler wait/IO, queue, Grant policy
or alias; canonical lexical/owning validation and dependency recovery are used.
The commit gate is the exact 23 reviewed paths plus this coordinator ledger,
after final metadata doc/English/diff checks. Unrelated review-results remain
untouched; Gateway R3a2, R3b/R3c, R4-R8 and whole-task admission/conformance and
closure remain pending. No push is authorized.

R3a1 committed locally as `05e7debf53e87ab3709e7d2090c0b3e7356244ca`
(`Bind ExtensionRef resolution to owning Route fields`), parent f65b863c7.
The exact staged 24 paths matched reviewed technical bytes plus the coordinator
ledger; staged diff and final metadata guards passed. Root verified all committed
technical bytes against immutable r3, an empty index and post-commit status only
`?? review-results/`. Stage and commit proofs remain in r3 verification. No push.

Root promoted the read-only R3a2 inventory to plan-only IMPL at this new HEAD.
The 30-path candidate has ten Gateway production files, twelve existing mirrored
tests and eight owning documentation/task files; no supporting Resources/Common
change is justified. Actual owning Route field loans must reach cold optional
keys and every early/final consumer, preserving positional holes and captured
scope hits/misses. Malformed-union gates and individual unsupported-target
positions, CORS fixture ordinals and real resolved-rule stamp tests require
explicit review. Inventory is not acceptance or execution; independent plan
approval and fresh baseline controls are required before edits. Root retains
07, the sole mutation lane, final gates/reviews and local commits.

R3a2's nine-point plan at 05e7debf5 received independent Astra SCOPED PLAN PASS
from `r3a1_fresh_astra`, restricted to the 30 proposed existing paths. The
reviewer traced all builders/factories, consumers and canonical dispatch; the
one-filter borrowed preflight slice must reuse existing identity/log/response
machinery and slice-based reservation, preserve leading policy/config precedence
and four-stage counts/order, and leave backend rejection selected-only. All
invalid descriptors have zero lookup, while valid sibling demand and missing
key recovery remain. Diagnostic names come only from the same indexed source
field; owning validation and effective locality stay canonical. No transport,
Resources/Common, Grant or hot-path mechanism expansion is approved.

Before execution, root froze all 30 immutable files at 05e7debf5 under
`tmp/resource-reference-contract/r3a2-baseline-20261008`. Baseline manifest SHA256
is `e8505004cc3d148af68e968c1f2db747e9a36bc7d8f9e501adac1f1c6fe94ff5`;
all 118 original attributed source tests and zero scoped ignores are recorded.
Fresh Gateway compiled list captured 5132 test identities, command exit zero
in 1.85 seconds; listing executed no test bodies. Root now authorizes the sole
implementer to execute this approved scope. The separate `r3a1_deep_astra` is
reserved as the fresh R3a2 final reviewer and did not author/approve this plan.
Final verification, two result rounds and local commit remain pending.

R3a2 stable intermediate targeted verification passed runtime 61, ExtensionRef
55, publication 25, adaptive 21, request filtering 28, resolution 9, HTTP match
12, GRPC match 9, access-log 15 and context 8 (overlapping selections). These
are intermediate actual execution, not the final candidate gate. The approved
adaptive fixture relocation preserves the same delegated rule and telemetry
assertions in actual resolved_rules.

The retained preliminary broad runtime run failed 1 with 419 passing: unchanged
dye inventory deserialized AiProxy {} despite required api. Static committed
bytes establish the stale fixture; no baseline test-body execution is claimed.
Independent Astra approved the exact test-only supporting path 31 replacement
with {"api":"openaiChat"}, preserving every inventory assertion/ID/ignore and
production behavior. Root froze its committed original bytes and identity
controls before authorizing execution. The final broad runtime rerun remains
required; original failure and intermediate provenance remain available.

R3a2 final r4 source was stable across all tests. Root reproduced raw log hashes
and results: runtime 424, exact dye one, publication 25, adaptive 21, request
filter 29, resolver nine, HTTP/GRPC match 12/nine, log 15, context eight, WAF 38,
early request 11 and plugin store 41; selections overlap. Source identities
122 -> 134 and compiled 5132 -> 5144 preserve every original test and ignore.
The immutable r1 31-path manifest is
91af55bf9d6e7c8cc7a6d2b554ccf93b1fc9cb85f0a257382b05975af164ff1a;
patch 783287d2fa34cff799095953c5b4c5a03798bed4bc5ff1870531fa47e28751bf.
Root workspace check passed cleanly (57.36 seconds), as did formatting and all
eleven guard checks. Clippy exited zero (61.842 seconds) but warned on the new
test's disallowed std::sync::Mutex; the diagnostic and generated summary do not
represent clean lint. Root deliberately blocked the commit gate.

The first fresh independent Astra result review reproduced the exact frozen diff,
identity and evidence controls and returned SCOPED BLOCK R3a2-F1 on that test-only
rule violation, with no production correctness blocker. A separately approved
precision correction replaces only that helper's Mutex and corresponding lock
calls, preserving every assertion/identity. Root authorized execution; fresh
stable affected verification, warning-free gates, replacement byte review and
the second deep review remain required before local commit. No push.

R3a2-F1 correction passed fresh r5 full runtime 424 and its exact cache regression
(one, included in runtime). Source 122 -> 134 and compiled 5132 -> 5144 identities
and zero scoped ignores remain intact. Only the three approved test mutex
expressions and truthful 04/06 evidence changed from immutable r1; production
and every assertion remained unchanged. Other focused r4 execution is retained
and distinguished from fresh r5. All 23 Rust hashes matched compile-start r5.

Corrected immutable r2 has manifest SHA256
be3bc21450701cd607d8ecda364b1651af57238f580ce4deb491e6d6b0b49554 and patch
7a0fa349d2f740ff3f5dcd74b2e64ff73fec7663a58eb4b19983d4d327627468.
The first fresh Astra reviewer renewed SCOPED PASS, resolving F1. A second
independent deep Astra result round reviewed the whole actual diff and consumers,
reproduced baseline/current/frozen hashes, source/compiled identities and raw
verification, and also returned SCOPED PASS with no blocking finding. Both
confirmed owning projection, policy/preflight priority, positional holes,
selected-backend scope, captured hits/misses and no new Grant authority.

Fresh coordinator r5 workspace check and Clippy passed in 33.13 and 25.579
seconds. Root and both reviewers inspected raw logs: zero warning/error
diagnostics. Formatting and all eleven guards passed; all 31 reviewed technical
hashes remained unchanged. The graded MUST scan confirms canonical validation
and dispatch reuse, no new secret/schema/transport field, request key formatting,
lock/snapshot/store, hot tracing, body log, queue/handler IO or alias/Grant policy.
The exact commit gate stages only these 31 reviewed paths plus this ledger after
final metadata checks. Unrelated review-results and the original issue remain
untouched. R3b/R3c, R4-R8 and final migration/admission/conformance/closure remain
open; no push is authorized.

R3a2 committed locally as 75a5363f52275a7e48252308f1f351c4a51504a7
(Validate Gateway ExtensionRefs through owning Route fields), parent 05e7debf5.
Root staged exactly the 31 approved reviewed technical paths plus this ledger,
reproduced every staged and committed technical hash against immutable r2, and
verified an empty index/clean tracked worktree with only unrelated review-results.
Final metadata docs/English/diff checks passed. Stage and commit proofs remain
under r3a2-verification-20261008-r1. No push was performed.

At this committed HEAD root promoted the prior read-only RequestMirror inventory
to R3b plan-only IMPL. The implementer must revalidate actual three-field coverage,
original owner/slot binding and native status priority before proposing an exact
allowlist. Narrow post-Grant cached presence inspection preserves absent backend
priority; invalid descriptors do no read/edge and Gateway denial precedes all
mirror arming. Disabled plugin namespace recovery and original child authority
remain intact. The next plan, baseline, execution and independent results are
pending; this inventory does not approve mutation or claim conformance.

R3b's nine-point exact 31-path plan received independent Astra SCOPED PLAN PASS
at 75a5363f5 (12 production, 13 mirrored tests, six owning documents). Supporting
route_utils and handler mod changes are necessary: late Service clearing would
remove earlier child/drop mirror edges, and the old omitted-namespace source
normalization queues default/mirrors instead of actual bare RawStore key mirrors.
The explicit old owner assertion correction preserves target/default and every
replace/delete assertion. Raw HTTP wake identity is retained while semantic
GatewayRouteIndex/target default projection remains separate; forwarding-policy
current/dropped wakeups and mirror-role exclusion must be verified. Other kinds
and global key semantics are outside this supporting correction.

Root froze all 31 original files and 291 scoped source identities, zero ignores,
under r3b-baseline-20261008. Manifest SHA256:
703c0f4069a9d38e2f9a81509c581b3f9ee702fee283ce86bbfcd742fa14cd00;
source identity SHA256:
7d1714e9d778f53da33da36a59565d984b6b0035f17e7272dc02b17f2a55a00b.
Fresh list-only baseline commands all exited zero: Resources 1925, Controller
1743, Gateway 5144. Listings executed no test bodies. Root reproduced original
current hashes and exact HEAD, then authorized the sole implementer to execute
the approved scope. Stable meaningful tests, workspace gates, two independent
actual-result reviews and local commit remain pending. No push.

The execution handoff needed a new agent turn after the returned plan; root
corrected the scheduling call and confirmed active execution. All 12 approved
production migrations are now drafted. The implementer is adapting original
fixtures to actual-owner construction without weakening assertions, then adding
boundary and worker-lifecycle evidence. No R3b Cargo execution has occurred yet;
this checkpoint records implementation progress, not verified correctness.
An independent read-only R3c descriptor inventory may proceed concurrently but
requires revalidation after this batch commits before any plan or mutation.

Preliminary actual verification reached Resources 1601 passed, doctests 27
passed/10 ignored, Gateway plugins 2254 passed/17 ignored, and Controller 924
passed/two failed. Earlier compile failures and both old HTTP owner expectation
failures remain recorded; no final acceptance is inferred. The newest dropped
child Service recovery test still requires execution. Original 291 scoped
source identities remain present, with 14 additions.

An independent Astra precision review approved a 32nd test-only supporting path,
route_backend_refs_test.rs: change only two shared-macro HTTP Service owner
expectations from normalized default to raw None. The other four route kinds
and every target/replacement/cleanup/status/recovery assertion remain intact.
Root froze its six tests and zero ignores at immutable HEAD before amended
EXECUTE; supporting file SHA256 is
7c9875268d38be9849d37908ffa81991905016ab75188f5df7301331ff684c1e.
The exact technical allowlist is now 32; root owns this ledger separately.
Root restarted the implementer with the approved amendment and final verification
requirements. Stable handoff, warning-free workspace gates, both actual-result
reviews and local commit remain pending. No push.

Final r4 focused verification passed: Controller 927 (including supporting six),
Resources 1601, Resources doctests 27/10 existing ignores, and Gateway plugins
2254/17 existing ignores. Source identities 297 -> 311 preserve all originals
and zero scoped ignores; compiled Resources/Controller/Gateway lists are
1925 -> 1929, 1743 -> 1749 and 5144 -> 5148 without removals. The actual raw
Service/EndpointSlice worker and dropped-child repair tests passed.

Root's tracing guard rejected the stale cold-function registration after the
private builder extraction. A separately approved precise 33rd-path amendment
renames only that registration to add_request_entries; no hot-path exemption or
pattern broadening. Root froze the original guard before execution. The revised
guard and all 18 existing Python self-tests passed; the original failure remains
recorded. Root independently froze all 33 technical paths for the first actual
Astra result review, manifest SHA256
d1ea9d4200eb595675b0a04adbeb34a75d76e1f5d679f27785fc6fc89ffdef7f,
patch a40e2485045e405df9a2c68a3bfa11f3b6a56c85e89c85a3a521ee3ba4805d95.

Root workspace check exited zero after 397.08 seconds, but reported one newly
unused production ResourceMeta import plus its summary. This is not a clean
gate: R3b-F1 remains blocking. Clippy and the first actual deep review continue
against frozen sources; correction, renewed verification/review, final metadata,
the second actual deep review and exact local commit remain pending. No push.

R3b-F1 is corrected solely by cfg(test) on the existing test-only import; only
that Rust file and truthful task04/06 evidence differ from frozen r1. Fresh r5
Gateway execution again passed 2254 tests/17 existing ignores; unchanged r4
Controller/Resources/doc evidence is retained explicitly. Corrected immutable r2
manifest SHA256 is
f109b460f6afc4d3ecf4a6598eb1c12a866864ba9601f81c5fbcc07c851f7c4b;
patch 9dc75514ceb79439128877a6509054c903235a7c88594ca88dbc19627b64d861.

Both independent Astra actual-result rounds now return SCOPED PASS on corrected
r2. The first resolved F1; the second reviewed the whole actual diff and callers,
not merely the import correction. Both reproduced every frozen/current hash,
exact baseline patch, raw test evidence, binary hashes, original identities and
ignore controls. No blocking correctness finding remains within this batch.

All 14 fresh root r2 gates passed with all 33 technical hashes unchanged.
Workspace check took 43.543 seconds and Clippy 38.917 seconds; raw logs contain
zero warnings/errors. Formatting and docs/SSA/metrics/Pingora/transform/reference/
unit-layout/tracing/English/diff guards passed. Root independently reproduced
every log hash and gate outcome. Earlier failed runs remain available.
The graded MUST scan confirms canonical reference validation and Grant reuse,
actual authored authority, no new serialized/schema/secret field, manager/queue,
handler IO, runtime key formatting/lock/snapshot/metric or hot tracing, and
preserved body/transport/failure contracts. Exact staging is limited to these
33 reviewed technical paths plus this coordinator ledger. The original issue,
unrelated review-results, R3c-R8, F1/F2 and live conformance remain open. No push.

R3b committed locally as 16e7be6dce9d9d7ffc9dd70fa6889a557043aace
(Bind RequestMirror references to owning resources), parent 75a5363f5. Root
staged exactly 33 reviewed technical paths plus this ledger; every committed
technical hash matches corrected r2. Post-commit proof confirms an empty index
and clean tracked worktree with only unrelated review-results. Stage/commit
proofs remain under r3b-verification-20261008-r1. No push was performed.

Root has started R3c plan-only IMPL at this new immutable HEAD. The independent
descriptor inventory must be revalidated against R3b's owner-aware builder:
Controller acquisition already binds the actual local ConfigMap selector,
while Gateway detached construction and weak descriptor-name validation require
investigation. Preserve compiler/body/transport bounds, original masked ordinals,
local-only semantics and existing dependencies; prove actual ConfigMap-to-plugin-
Route/Gateway worker recovery. No R3c mutation or extra scope is authorized before
the next plan review, frozen baseline and explicit EXECUTE. Generic dependency-
status reads remain separately tracked in R4. The original issue remains open.

R3c independent Astra plan review returned SCOPED PASS for exactly 15 paths.
The material-free unsupported-stage contract includes Resources secondary body
validation, and actual descriptor loans must gate request validation before
cache lookup or construction. Root froze 235 original test identities and zero
ignores (the preliminary 233 scan missed two parameterized Tokio tests).
All three compiled baseline list-only commands exited zero: Resources 1929,
Controller 1749, Gateway 5148. No test bodies ran; all 15 source hashes remained
unchanged. Plan/review and baseline artifacts are under r3c-plan-20261008 and
r3c-baseline-20261008. Root now authorizes the sole implementer to EXECUTE the
reviewed scope. R4 identity/status boundaries remain open; no live conformance
or whole-task completion is claimed. Root retains both actual-result review
rounds, workspace gates, exact staging and local commit. No push.

R3c implementation is frozen across the approved 15 technical paths. First actual
independent Astra review returned SCOPED PASS, verifying ownership-before-material,
cache gates, same-entry construction, unsupported-stage material-free ordering,
real worker recovery, exact source/test/log/binary hashes and documentation limits.
Resources resource/transcoding tests passed 1602/13; doctests 27/10 existing ignores;
Controller processor 929; Gateway plugins 2260/17 existing ignores. Source test
identities 235 -> 245 preserve every original assertion (seven original files are
verbatim prefixes). Full compiled IDs 1929 -> 1931, 1749 -> 1751, 5148 -> 5154
remove none. The r2 new-fixture failClose spelling failure was corrected to the
existing fail-close enum without changing assertions; the raw failed log remains.
Final retained-source evidence is explicit in identity-comparison-r4.json.

All 14 root gates passed with unchanged technical hashes; workspace check took
47.479 seconds and Clippy 80.239 seconds, both zero warnings/errors. Root reproduced
all raw log hashes and gate outcomes. Frozen r1 manifest SHA256:
40883af8a0dfd6f48239967f275ef248a4e464b1d711fb2d4787b91bcf2f2888;
patch ac474c8bdb7085a0f55a56f75c459e5868a456b2fc09893f583d5432f4917720.
Only a final evidence update to the four approved documents is now authorized,
followed by a new immutable freeze and second actual deep review. No Rust changes,
second-round acceptance, commit or conformance completion is claimed yet. R4
raw-None lookup/status boundaries and remaining batches stay open. No push.

Second R3c actual review BLOCKED on R3c-F1: although owner validation gates
compilation/cache, add_request_entries still clones resolved descriptor bytes
via entries.to_vec before rejecting invalid ownership; compile_plugins_with_cache
also clones the entire EP earlier. The no-material-consumption contract includes
copying bytes, so neither pre-existing behavior nor softer documentation closes
this defect. All other reviewed source/evidence checks passed. Exact r2 remains
frozen and uncommitted; the implementer is investigating a correction PLAN ONLY,
including upstream generation/policy clone paths. Any added path needs an exact
reviewed scope amendment and pre-edit baseline. Preserve rate-limit aliases,
ordinals, masks, diagnostics and valid siblings. First scoped PASS and clean gates
remain historical evidence; final acceptance is blocked until corrected and reviewed.

R3c-F1 exact19-path correction plan passed independent Astra review. The four
supporting originals are frozen at HEAD, including18 original publication tests
and zero ignores (total253 original scoped tests). One actual-owner cold-copy
helper will gate publisher/generation/compiled configuration payload copies;
borrowed canonical rate-alias validation replaces runtime whole-entry cloning.
Raw input retention/equality serialization remains input bookkeeping; masked-view
zero-copy does not describe the earlier eligible source generation. No Grant,
schema, queue, transport or hot-path change is approved. Root now authorizes
EXECUTE within the amended19 paths; actual acceptance remains blocked pending
corrected verification and renewed deep reviews. No staging, commit or push.

R3c-F1 corrected19-path implementation is frozen as r3c-final-20261008-r3,
manifest13f024a2c95cbd5520ba5c6418690963eb46bdeaaee2cdcadb34dc913c11a38b,
patch9730d9d55fb969b7d855b923236a0a312e290e6e0c96044620aa1fc10042a26c.
Fresh corrected tests passed Resources1602 plus transcoding13, doctests27/10
existing ignores, Controller929 and Gateway2264/17 existing ignores. Source IDs
253 -> 267 preserve all eight original test files verbatim and prior R3c additions;
compiled IDs1929 -> 1931,1749 -> 1751,5148 -> 5158 have no removals. Raw failures
and superseded review evidence remain available, not current acceptance.

All14 renewed root r2 gates passed against unchanged19 technical hashes; check
42.548 seconds and Clippy65.042 seconds with zero warnings/errors. Root reproduced
all raw log hashes and the before/after/current/frozen manifests. A newly spawned
confirmed gpt-6-astra reviewer, r3c_f1_fresh_result_astra, independently reviews the
whole corrected result without participation in either plan review. Its final
verdict and a second independent actual-result round remain pending; no commit yet.

Both renewed independent actual R3c reviews return SCOPED PASS on the same frozen
r3 candidate. Fresh reviewer r3c_f1_fresh_result_astra did not participate in plan
review; second reviewer r3a1_fresh_astra independently confirms its R3c-F1 finding
resolved through the complete construction chain. No scoped blocker remains.
Both checked exact19 source hashes, baseline patch, original assertions, raw test
and binary hashes, all14 renewed gate results and precise documented boundaries.

Root graded MUST scan: canonical identity/borrowed rate dependency reuse, actual
owner-before-payload construction, valid siblings/ordinals and policy preservation;
no new schema/serialized field, Grant authority, queue/handler IO, hot tracing,
metrics/locks/snapshots, body/transport mechanism or sensitive logging. English
content and mirrored unit layout pass. Stage only the19 reviewed technical paths
plus this coordinator ledger; unrelated review-results is excluded. Original issue,
R4-R8 and F1/F2 remain open. Commit is authorized locally; never push.

R3c committed locally as ca7bdaf79bed40287505287345e5ddb5f443ae84
(Bind descriptor construction to owning plugin references), parent16e7be6dc.
Exact20 staged paths contained19 frozen technical files plus this ledger; root
verified every committed technical hash, clean tracked tree and empty index.
Only unrelated review-results remained untracked. No push performed. Stage and
commit proofs remain under r3c-verification-20261008-r1.

Root starts R4a PLAN ONLY at this new HEAD: revalidate omitted-namespace Service
wake/cleanup/policy notification keys for GRPC/TCP/TLS/UDP Routes. Preserve exact
raw owner keys while semantic target/index namespace remains effective default;
HTTP's reviewed R3b correction is precedent, not a reason for blind replacement.
Require real event/worker/source-generation tests and independent plan review before
edits. Admin namespace admission, EP semantic collisions and generic readonly status
remain separate R4 work; candidate evidence is under r4-candidate-20261008.

R4a exact14 plan passed independent Astra review. Root froze14 originals,13 original
IDs/0 ignores and fresh Controller compiled1751 listing (no test execution). The
three specified owner-expectation amendments are approved; every other assertion
remains. Root authorizes EXECUTE with actual raw ResourceRef reused throughout
Service registration/replacement/delete and current/dropped policy notification,
while semantic target/index defaults remain unchanged. Baseline/plan artifacts are
r4a-baseline-20261008 and r4a-plan-20261008. No broader R4 work or push authorized.

R4a candidate passed931 Controller tests (source13 ->15; compiled1751 ->1753,
no removals/ignore changes) and all14 clean root gates. First actual independent
review found R4a-F1, a bounded verification gap: new Service lifecycle matrix omitted
ordinary same-UID update. Production semantics and evidence otherwise passed.
Root authorizes adding the already planned update case within the existing new test,
with changed RV/data and real raw-key worker/publication assertions. No production
or scope expansion. Earlier new-fixture delay-queue and TLS hostname failures remain
recorded. Corrected focused verification and both actual reviews remain required.

R4a-F1 corrected: same-UID Service update changes RV1 ->2 and targetPort8081,
then real Service and raw Route workers prove publication/current Route identity
and both dependency directions for all8 cases. Corrected Controller931 tests pass;
source13 ->15 and compiled1751 ->1753 retain all originals/ignores. All14 renewed
root r2 gates pass, check10.884s/Clippy14.034s, zero warnings/errors. Root reproduced
raw logs and unchanged before/after/current/frozen14 hashes.

Both independent actual Astra reviewers return SCOPED PASS on corrected r2,
manifest0ea2c205ae0c0dd58f080479dc5637ba1f4a4cadf9a260d76cf61e09cc0997bd,
patchf90ae43feb213c8b2f319e493fc4c96233de473747b7b5b9ed882bc3c0db596f.
First reviewer independently resolved F1; second reviewed the full actual diff,
callers, lifecycle and evidence. No scoped blocker remains. Root graded MUST scan
confirms canonical raw-owner reuse, semantic target/index defaults and Grant
unchanged, no added queue/handler IO/schema/logging/metrics/hotpath work, preserved
assertions except3 reviewed owner expectations. Stage exactly14 reviewed technical
files plus this ledger; unrelated review-results remains excluded. Local commit
next; no push. Broader R4-R8/F1/F2 and original issue remain open.

R4a committed f8bfbf6a52aa509db50366fe3f6521e931d56835 locally, parentca7bdaf79.
Root verified exact14 committed technical hashes,15-path staging, empty index and
clean tracked tree. Only unrelated review-results remained. No push. Proofs are in
r4a-verification-20261008-r1.

R4b now PLAN ONLY: revalidate generic EdgionPlugins dependency-string status reads
against actual current fields and inventory all diagnostics they currently own.
CredentialInjector suppression is coupled to this scanner; replace with exactly
one correct readonly missing/denied diagnostic, not silent loss. Preserve persist-off
LinkSys authority/memory semantics. New plan, independent review and baselines must
precede edits. Admin namespace and EP source-collision contracts remain separate.

R4b exact10 plan passed independent Astra review. Root froze136 original tests/zero
ignores and fresh compiled Controller1753 list-only baseline. EXECUTE is authorized
within reviewed scope: actual-field readonly classifiers replace registry-string
status authority, Injector missing/denied diagnostics become owner-derived, exact
aggregate dedup and deterministic priority remain. Observer scope is canonical
boundary calls, with same-namespace authorization distinct from Grant-store query.
Preserve persist-off/memory/expiry/binding/global-switch behavior and readonly edges;
prove real Secret/ConfigMap lifecycle including ordinary same-UID updates. No broader
R4 collision/admission/conformance claim. Artifacts r4b-plan/baseline-20261008.

R4b focused935 tests passed, source136 ->140 and compiled1753 ->1757 retain originals.
First actual independent review found only mandatory layout failure R4b-F1: three
source-local cfg(test) observation statements are test implementation in src. The
other13 root gates passed, check/Clippy warning-free; originalinputs remained frozen.
A precise two-file correction plan moves observation implementation into mirrored
tests through a transparent expression macro, keeping production expressions and
all observer assertions unchanged. No guard exception/weakening or runtime overhead.
Independent amendment review is required before execution; no commit acceptance yet.

R4b-F1 correction passes the unmodified layout guard: mirrored test macro records
then evaluates each original expression exactly once, while the non-test macro
expands directly to that expression. First independent actual reviewer returned
SCOPED PASS on corrected frozen r2, preserving all prior full-review conclusions.
Fresh Controller935 tests pass; source136 ->140 and compiled1753 ->1757 retain
originals/ignores. All14 renewed root r2 gates pass, check17.374s/Clippy22.279s,
zero warnings/errors. Root reproduced all rawlog hashes and stable10-file manifests.

Corrected manifest c4a4e2494b7121cf2a3bab7408b2fb018c3ffe79c8e127f90d48040dfbdfad37;
patch c3527e82efdc4364934d163bc3ef6b527a1e429e252ef9ba662df8b9ade9accf.
After a user-turn interruption removed prior agents from the live roster, root
started new confirmed gpt-6-astra r4b_second_astra for the required second whole
actual-result review against the same immutable evidence. No result is presumed;
commit remains gated. No push, no broader identity/conformance acceptance.

Second independent Astra whole-result review returns SCOPED PASS for corrected
R4b r2. Both reviews independently checked classifier coverage, restored Injector
diagnostics, readonly/stale-edge safety, original assertions, test macro production
transparency, rawlogs/binaryhashes and all14 clean gates. No scoped blocker remains.
Root graded MUST scan confirms canonical reference/Grant ownership, unchanged
credential memory/expiry/binding semantics, no new runtime observer or logs/metrics,
queue/handler IO/schema/transport changes, and compliant mirrored test ownership.
Stage exactly10 frozen technical paths plus this ledger; review-results is excluded.
Original issue and remaining R4-R8/F1/F2 remain open. Local commit, no push.

R4b committed5225c3e15d9e2eec5c6448935d7a226326b9b9e6 locally, parentf8bfbf6a5.
Exact11 paths contain10 frozen technical files plus this ledger. Root reproduced
all committed hashes, clean tracked tree and empty index. Only unrelated
review-results remained; no push. Stage/commit proofs are in r4b-verification-20261008-r1.

R4c PLAN ONLY starts at this HEAD: verify Admin namespaced-write admission adopts
its authoritative URL namespace when body namespace is omitted, preserves explicit
mismatch rejection and strict FS/etcd storage-content ingest identity. This does not
authorize ingest backfill, global key normalization or bare EP semantic collisions;
those remain separate. A fresh implementer reads original rules/knowledge and must
return an exact independently reviewed plan before edits. Root07 remains exclusive.

R4c independent Astra plan review passes for three technical paths: namespaced
Admin admission, its new mirrored tests, and the owning Admin identity guard.
Bind omitted/null namespace from URL before typed dispatch while preserving raw
source fields and validation/precondition ordering. Reject malformed metadata and
explicit namespace contradictions. Existing writer merge retains current identity;
no claim of repairing malformed stored records or an omitted-name defect follows.
Compiled Controller baseline is 1757 tests. Implementation proceeds in one lane;
R4d semantic source-identity investigation is read-only and not an approved plan.

R4c completes scoped verification: API167, FS21 and etcd13 tests pass without
warnings; compiled identities1757 ->1762 retain every original. All frozen
pre-existing test files remain byte-identical. First run passed with four new
unused-return warnings; warning cleanup briefly broke three assertion expressions.
Both raw rounds remain recorded, and the corrected final run is clean.
Fresh independent actual review and second Astra deep actual review both return
SCOPED PASS. Root independently verified all14 gates, raw logs, test binary and
stable three-file hashes; workspace check49.404s and Clippy37s are warning-free.
Graded MUST scan: one admission helper, no reference/Grant authority change, no
handler IO/queue/logging/schema/secret-transport or Gateway hot-path change.
Stage only the three reviewed technical files plus this ledger; no push. This
acceptance covers in-process admission/persistence, not live conformance or the
remaining reference-contract scope. Evidence: r4c-verification-20261008,
r4c-root-verification-20261008 and r4c-final-20261008 under the ignored task area.

R4c committed fc4abfee03923241176bcd851bcff85b19125c45 locally, parent5225c3e15.
Root verified all committed technical hashes and clean tracked/index state;
unrelated review-results remains untouched. No push.

R4d starts PLAN ONLY at this HEAD. Read-only inventory confirms EP raw name versus
semantic default/name mismatch in Controller target lookups and Gateway publication.
Global/Gateway ordinary missing views are dropped, so ambiguity cannot be encoded
as absence without bypassing security plugins. The proposed design must distinguish
Missing/Unique/Ambiguous in cold target resolution, preserve exact raw identity and
rate-limit scopes, prevent cross-source LKG reuse, and recover through existing
semantic dependency wakes and adaptive notifications. Collision semantics and the
exact allowlist require independent plan review before any implementation.

R4d independent Astra plan review returns SCOPED PASS for one coherent30-path
integration (13 production,12 tests,5 documentation). The approved EP-only
collision rule distinguishes Missing/Unique/Ambiguous before reads; explicit
empty namespace is not an alias. Ambiguity selects no candidate material or
metadata, publishes a non-retainable denial surviving Bypass, and reports only
ResolvedRefs=False without dropping Route slots. Collision clears semantic LKG;
recovery uses the survivor current generation. Same-raw-key UID-insensitive LKG
outside collisions remains accepted under D0030. Tests must include prior LKG,
collision and invalid-survivor recovery. Root authorizes implementation after
baseline capture; raw stores/rate scopes/exact deletion and other-kind source
contracts remain unchanged. Plan: r4d-plan-20261008 in ignored task artifacts.

R4d scoped implementation and both actual-result reviews pass. Shared EP target
classification, Controller Route-only collision diagnostics, Gateway sealed denial,
raw-owner retention/removal and adaptive semantic notifications satisfy the approved
contract. Review corrected a cfg(test) import and a new reason-priority tie;
ambiguity now ranks9 without changing old ranks, with reversed-order regressions.
Compiler/fixture failures remain recorded; all final focused suites pass:
Controller942; Gateway plugins2268 with17 existing ignored; resolver8; adaptive22;
classifier2. Compiled identities1931->1933 /1762->1769 /5158->5164 retain originals;
all11 existing allowed test files remain verbatim prefixes. OPTIONS coverage is
real-Session preflight invocation, backend coverage is Context cache pinning and
delegation coverage is effective owner demand, combined with inspected unchanged
production wiring; no full live pipeline or conformance claim.
All14 root gates pass, check51.25s/Clippy60.499s warning-free. Root reproduced
rawlog/binary/list hashes and the30-path frozen manifest. Graded MUST scan confirms
no raw-store identity, Grant authority, queue/handler IO, schema/secret transport,
request-path allocation/locking/logging or rate-scope changes. Ambiguous denial
contains no candidate material; unchanged raw source/prepared snapshots are not a
process-wide zero-copy claim. Stage only reviewed technical changes plus this
ledger, excluding review-results; local commit only. Evidence: r4d-verification,
r4d-root-verification and r4d-final directories under ignored task artifacts.

R4d committed b872bc8c95299115e4855548c4a87c27e575c950 locally, parentfc4abfee.
Commit contains29 changed technical files plus this ledger; one allowed existing
EP test file needed no edit. Root verified all30 candidate hashes in HEAD, clean
tracked/index state, unrelated review-results untouched, no push.

R5a begins PLAN ONLY: actual GatewayClass.parametersRef validation must be shared
across Controller resolution, Gateway listener startup and publisher scope selection.
The existing publisher already rejects wrong G/K and any namespace; other consumers
must not read the same-named EGC first. Revalidate original authority and freeze a
bounded independently reviewed plan before edits. Candidate R5b policy target
identity/atomicity remains read-only inventory until R5a commits.

R5a independent Astra plan review passes the exact14-path batch. A borrowed actual
GatewayClass parameters adapter validates exact G/K, strictly absent namespace,
and lexical cluster target name before every target read. Preserve absent built-in
defaults, invalid Controller InvalidParameters/startup skip/publisher unavailable,
valid-unwatched Controller Skip and existing missing-target recovery. No new owner
metadata strictness, Grant/schema/source-version/listener-reconstruction or process
configuration change. Baseline capture precedes implementation; old tests stay
verbatim. Separate stale selected-class/SupportedVersion documentation claims are
recorded under r5a-plan-20261008 for final F1 reconciliation, not silently rewritten.

R5a implementation and both independent actual reviews pass. Adapter1 + owner-loan
compile-fail1, Controller11, startup2 and publication23 tests pass warning-free;
compiled identities1933->1934 /1769->1771 /5164->5166 retain all originals and
old test files remain verbatim prefixes. One initial Gateway fixture compile
failure is retained; only new fixture Arc serialization/delta arguments changed.
All14 root gates pass, check46.211s/Clippy81.077s, no warnings/errors and stable
14-path hashes. Root reproduced raw logs, list/binary hashes and old tests.
Graded MUST scan confirms one owning parameters classifier, validation before
lookup, unchanged Grant/source/schema/secret/queue/handler IO/hot-path semantics.
Existing startup warning is still cold-path; no new observability mechanism.
Coverage is config-name selection, actual Controller queues and immutable scopes,
not live listener reconstruction or conformance. Stage reviewed changes plus this
ledger; local commit only. Evidence under r5a-verification, r5a-root-verification
and r5a-final ignored artifacts. Remaining R5b/R5c/R5d/R6-R8/F1/F2 stay open.

R5a committed4917ce7673a28ed47ff682d3295d1fb1eaf23236 locally, parentb872bc8c9.
Exact15 paths include14 reviewed technical files plus this ledger; committed
hashes, clean tracked/index state and untouched review-results verified. No push.

R5b begins PLAN ONLY at this HEAD: actual-owner BTP/EBTP target validation must
reject illegal slots atomically and align effective missing-owner namespace
without altering raw source keys or deletion. Revalidate Service unbounded/idempotent
fanout versus EdgionBackend max16/no duplicates, native strict groups, finite
custom Service core alias, existing BTP invalid occupancy and dependency recovery.
No CA/material/health-check TLS/ConfigData/Grant expansion. Exact plan must receive
independent Astra approval before edits. R5c IpList and R5d WAF lexical/owning gaps
remain recorded read-only candidates; parent/allowedRoutes/reverse-consent checks
found no new semantic defect, and ProxyProtocol currently validates before reads.

R5b independent Astra plan review passes the frozen17-path scope; EXECUTE authorized.
Actual-owner policy target loans preserve strict native versus finite custom group
semantics and raw owner keys. Identity-invalid EBTP returns before both target reads
and the BTP hostname registry scan; valid sibling recovery dependencies survive.
Native BTP invalid occupancy remains distinct from EBTP atomic eligibility. Service
fanout stays unlimited/idempotent; AI-only16/unique limits remain unchanged. The
sole existing-test amendment is the reviewed BTP status helper invocation; all
assertions and identities remain. Fresh compiled baselines1934/1771/5166 captured
before implementation. No Grant, TLS material, schema, queue or IO expansion.

R5b implementation and both independent actual-source/frozen-evidence reviews
pass. Focused Resources loan1/schema18/doc2, Controller BTP15/EBTP27 and Gateway
TLS27/traffic53 pass on the first candidate, zero ignored or warnings. Compiled
identities1934->1936 /1771->1774 /5166->5168 retain every original. Five old test
bodies remain original prefixes after the sole approved BTP invocation amendment.
Root reproduced all17 source hashes, raw log/list/binary hashes and original HEAD
snapshots. All14 root gates pass, check88.584s/Clippy89.308s, stable source hashes.
Graded MUST scan confirms canonical actual-owner references, no Grant/material
authority expansion, unchanged schema/redaction, serial queues, handler IO and
request lookup paths. BTP invalid occupancy and atomic target-local snapshots
remain intact; Service fanout/duplicates and AI-only limits are preserved.
Zero target/hostname reads for invalid identities is control-flow proof; recovery
tests use direct handler calls, not actual queued events. No live or conformance
claim. Evidence: r5b-verification, r5b-root-verification and r5b-final ignored
artifacts. Stage exact17 technical paths plus this ledger, local commit only.

R5b committed 7e6c675ebad2feb4ede29cc89bc243743740e725 locally, parent4917ce767.
All17 technical hashes verified in HEAD; index/tracked tree clean before this
ledger continuation, unrelated review-results untouched. No push.

R5c begins PLAN ONLY: revalidate actual HTTP/stream/Condition/RAP IpList fields,
invalid lexical identities and stream missing-owner namespace dependency mismatch.
Use existing canonical identity and condition ownership machinery; preserve Grant
allow/deny polarity, raw source keys, disabled-slot contracts and pinned runtimes.
WAF/Selector/Region/ProxyProtocol stay separate. Independently review exact scope
and regression requirements before implementation.

R5c investigation splits implementation into serial complete field families.
R5c1 covers ConditionSet and RAP direct-IP actual-owner loans, Controller
resolution/status/dependencies and existing resolved-payload Gateway enforcement.
R5c2 covers standalone HTTP/stream IpRestriction (including TLS and GCIR) across
Controller and Gateway constructor/cache ownership seams. The latter cannot be
claimed complete from a Controller-only migration. R5c1 remains PLAN ONLY pending
exact independent Astra review; neither split changes existing failure policies.

R5c1 plan frozen for independent Astra review:15 paths, existing condition
catalogue extended to IP and direct RAP fields. Explicit review items include
IP enabled-only versus credential supported-stage activity, sorted RAP profile
warning order, PartialGood versus FailClosed, and Kind:raw-resource-key dependency
encoding. No implementation before review.
Residual ConfigData inventory confirms RAP URL document configRefs, non-WAF
Selectors (RAP/Canary/GCIR), and RegionRoute overrideRef remain outside R5c1/R5c2
and R5d. Track them as prospective R5e, requiring its own bounded plan/reviews.
ProxyProtocol already validates lexical identity before reads; its remaining
owning-adapter unification is not a new lexical-lookup defect. Evidence remains
in ignored r5-configdata-residual-inventory-20261008.md.

R5c1 independent Astra PLAN PASS; EXECUTE authorized for the exact15 paths.
Preserve IP enabled-only activity and unchanged credential stage filtering;
identity invalidity precedes reads, Grant queries and dependencies. Missing or
wrong-type targets stay unavailable, while present right-type cross-namespace
targets retain denied-before-disabled precedence. RAP warnings stay bound to the
actual entry with rule order, URL-before-IP and cap64 intact; sorted profile keys
replace unspecified HashMap traversal order. Ordinary unresolved behavior and RAP
PartialGood remain distinct. Fresh baseline/original capture precedes mutation.

R5c1 scoped amendment independently approved by Astra: add ref_grant/mod.rs
as path16 solely for a pub(crate) re-export of the existing pure ConfigData
permission decision. The module was private; the helper was already crate-visible.
This enables the approved token check_query plus canonical decision without
duplicating policy or widening external visibility. Capture its HEAD original
before editing; all other plan constraints remain unchanged.

R5c1 implementation and both independent final reviews pass. The existing
condition catalogue now owns ordinary and RAP direct/condition IP slots; token
validation precedes registration, target and Grant reads. Raw Kind:resource-key
dependencies align clearing, IP resolution and URL expansion. IP enabled-only
activity, credential stage filtering, ordinary FailClosed and RAP PartialGood
remain distinct. Warnings retain entry ownership, URL-first order and cap64.
Review removed unbounded pending warnings and extra CIDR copy/content validation
from status; the existing store clone remains. First compilation caught an
exhaustive slot arm and borrow-scope error, corrected with raw failure logs retained.
Focused Resources10, Controller2+115, Gateway45+18+17, sealing3/body2 and loan docs2
pass, zero ignored. Compiled identities1936->1939 /1774->1779 /5168->5171 retain all
originals; five old test bodies remain unchanged. Root fmt changed only three new
Gateway test blocks; preformat evidence is retained, affected45/18/17 tests and
Gateway list reran, and the final16-path freeze was refreshed and independently
verified. All14 root r2 gates pass with stable source and zero warnings/errors.
Graded MUST scan confirms no material/Grant authority expansion, schema/redaction
change, new handler IO/queue or request-path work. Tests demonstrate counted invalid
resolver access and direct handler recovery, not live queue/network conformance.
Stage exact16 technical paths plus this ledger, local commit only. Evidence under
r5c-verification, r5c1-root-verification and r5c1-final ignored directories. R5c2,
R5d/R5e, R6-R8 and F1/F2 remain open.

R5c1 committed 291a4e08ee0d63303135f11ce564f420feeb482c locally, parent7e6c675eb.
All16 technical hashes verified in HEAD; clean tracked/index state before this
ledger continuation, unrelated review-results untouched, no push.
R5c2 begins PLAN ONLY for standalone HTTP/connection/TLS/GCIR IpList references
across Controller and Gateway cold construction/cache seams. Preserve allow intent
versus deny-set omission, actual EP/ESP authorization and raw owner keys; unrelated
Selector/Region/URL/WAF target semantics remain separately tracked.

R5c2 exact plan submitted for independent Astra review:37 paths (18 production,
14 tests,5 documentation). Actual EP/ESP standalone loans cover four constructor
families and generation cache builders together. Shared Controller IpList resolution
reuses R5c1 with explicit fixed target/source-role guards. Existing blank-name
init failure remains; newly invalid target slots omit keys while preserving allow
intent and valid siblings, whereas broken owning bindings fail closed. Detached
convenience APIs may survive only as test fixtures; production requires actual
owners. Disabled Controller processing versus inert Gateway entries and raw rate
keys remain unchanged. No mutation before independent plan approval.

R5c2 independent Astra PLAN PASS; required clarifications are incorporated and
EXECUTE authorized for37 paths. Test wrapper bodies live only in mirrored test
modules. Shared resolver checks exact edgion.io EP/ESP source and fixed ConfigData
target before access. Owner/stale-binding failure is independently classified
from target-slot invalidity, never guessed from overlapping error variants.
Immutable generation caches/raw rate keys and one live ConfigData snapshot per
decision remain intact. Freeze originals and fresh1939/1779/5171 baselines before
mutation; all old tests stay unchanged. No Selector target migration is authorized.

R5c2 fresh baseline capture completed at1939/1779/5171 with zero exits. The shared
standalone adapter is implemented; Controller/Gateway migrations and focused
verification remain in progress. Independent actual-code review has begun on the
shared layer; no final verdict or commit acceptance is recorded yet.

Read-only R8 native TLS inventory is complete (ignored r8-native-tls-inventory-
20261008.md). Candidate gaps concern certificate registration before identity
validation, frontend CA name validation, and BTP CA names/owner defaults. Existing
frontend pre-projection validation, complete-trust protection and strict local BTP
client-option grammar are counterevidence, not defects. Revalidate against the
then-current HEAD before a separate R8 plan; no TLS mutation is authorized here.

Read-only F1 documentation inventory (ignored f1-doc-inventory-20261008.md)
records six reconciliation items against committed R1-R5c1: native BTP required
group/kind mislabeled as defaults; conditional GatewayClass parametersRef fields;
stale SupportedVersion claims; ordinary versus RAP condition failure wording;
historical issue gates and proposed-alias labels. Final cleanup must reconcile
English/Chinese tables together and clarify Grant name omission versus explicit
empty, without turning finite custom aliases into universal native defaults.

R5c2 live verification inventory found static HTTP IpList traffic assertions and
Grant-only status transitions, but no four-family IpList-only hot-update scenario
or Grant-revocation traffic assertion. Existing GCIR Selector-only updates use
inline profiles; CanaryHotSwap also republishes its owner. These are explicit F2
coverage gaps, not acceptance evidence for live IpList recovery. Exact existing
runner commands are in ignored r5c2-live-verification-inventory-20261008.md.

R5c2 two independent production reviews found no semantic blocker. Review-driven
test additions cover actual ESP Grant source isolation/add/revoke, unconditional
ResolvedRefs transitions, initially missing live targets, blank-name constructor
failure, and side/order binding drift. Raw first-run failures are retained: a new
GCIR fixture omitted enable; Controller exposed a returned-token borrow lifetime
and new-test call/lifetime mistakes; Gateway missed a StreamPluginEntry import.
Resources r3 passes90/0/0 and Controller r2 passes698/0/0. Controller r2 warnings
and Gateway compilation errors were corrected; final reruns/review/gates remain
pending. These intermediate results do not establish batch acceptance.

R5c2 first root workspace check/Clippy pass without warnings, but tracing and
unit-layout guards block commit. Independent Astra amendments authorize path38
check_gateway_no_tracing.py solely to rename three existing cold registrations,
and removal of an unused test-only IP fallback from production plus its mirrored
helper. Six detached-helper caller files contain no IP entry; no cache-warming
mechanism or guard exception is needed or approved. Add a focused ownerless IP
rejection regression, preserve old bodies, refresh evidence and rerun gates.

R5c2 final38-path candidate passes both independent final reviews and all14 root
r2 gates with unchanged source and zero warnings/errors. Four standalone IpList
families now borrow actual EP/ESP fields through one finite adapter, share the
condition permission resolver, and compile only validated, permitted target keys.
Owner/binding failure stays separate from unavailable individual targets; authored
allow intent, deny-set omission, initialization, disabled-entry behavior, raw rate
identity and immutable generation caching are preserved. ESP dependencies use raw
source keys and retain valid missing/denied recovery edges. No new material role,
schema field, Grant authority, handler IO/queue or request-path work was added.
Focused Resources90, Controller698, Gateway2288 pass;17 Gateway ignores are pre-
existing. Reference compile-fail docs30 and the strengthened escape example pass.
Tracing regression tests18 pass; no guard exception or cache-warming seam was added.
Compiled identities1939->1944 /1779->1784 /5171->5187 lose no old tests;12 original
test files remain exact prefixes. Root final manifest SHA256 is
212bd1e600c6e16436645eb15afad0c2398f56d0b98eac1bf1aa1db158484ce9.
Earlier failures and pre-correction freezes remain under ignored evidence paths.
Stage38 technical paths plus this ledger and commit locally, without push.
Direct handler/store and generation tests do not establish live queue/network or
conformance acceptance. R5d/R5e, R6-R8 and F1/F2 remain open. Unrelated review-results
and the concurrent untracked abstraction-simplification task are excluded.

R5c2 committed745850e98f58c65b5128faa6719954ec71934a0f locally, parent291a4e08e.
All39 staged file hashes match HEAD; tracked/index state is clean before this
continuation. Unrelated untracked artifacts remain untouched, no push.
R5d begins PLAN ONLY for finite WAF policy/Selector/bundle owning references.
Revalidate the prior inventory on this HEAD; preserve one-lock source capture,
distinct EP versus WafPolicy authorization, required/optional semantics and LKG.

R5d exact19-path plan (8 production,7 tests,4 docs) is submitted for independent
Astra review. Small borrowed owning facades replace reconstructed keys without
another Prepared catalogue. Single-lock capture validates the captured policy
identity before child access; Gateway checks actual EP policy identity before
cache use and validates selected bundle transport without new Grant authority.
One old over-limit test needs its unchanged EP fixture moved before capture and
only the string snapshot call replaced; assertions/IDs remain frozen. Existing
bundle-denial tests do not prove Grant add/revoke source isolation, so explicit
transitions are required. No production mutation or Cargo before plan approval.

R5d independent Astra PLAN PASS; EXECUTE authorized for the exact19 paths after
original/fresh1944/1784/5187 baseline capture. Preserve shape-vs-identity failure
boundaries, actual-owner authorization, one capture lock/copy bounds, optional and
required semantics, LKG/pins and the sole documented old-test call amendment.
The allowlisted ConfigData architecture must correct its stale claims of always-
true own ResolvedRefs, IpList-only PartiallyInvalid and EP/ESP-only Grant sources
to include actual WafPolicy bundle behavior. Final actual reviews/gates remain open.

Read-only R6 test inventory revalidates the raw Service DNS producer permission
gap and same-fingerprint EndpointSlice skip without Service UID comparison at
745850e98. Existing mock-API/skip/preparation helpers are identified; forced SSA,
LKG and failed-preparation fallback remain counterevidence. No async fencing
design or next implementation scope is inferred. Evidence is in ignored
r6-service-dns-test-inventory-20261008.md for later independent planning.

R5d production migration has provisional source approval from both independent
reviewers; final tests/docs and frozen evidence remain pending. Resources WAF
r1 passes41/0/0. Explicit EP policy/Selector and policy-owned bundle Grant
wrong-source/add/revoke transitions are required before final acceptance.
An initial observation of newly eager Grant reads was retracted after inspecting
the old helper: it already computed grant_allows before the permission decision.
Preserve existing behavior rather than introduce an unrelated optimization.

R5d final19-path candidate unifies actual EP policy/Selector loans and actual
WafPolicy ConfigData bundle loans. Invalid identities stop before owned target
access, authorization and dependencies; valid missing/denied edges survive.
Single-lock capture, required/optional profile outcomes, Selector fallback,
selected transport checks, raw source keys and Gateway LKG/pins are preserved.
There is no new Prepared catalogue, schema surface, queue or request-path work.
Focused verification passes286 unit tests and2 actual-owner E0505 doctests.
Compiled identities1944->1947 /1784->1790 /5187->5189 lose no old tests. Four
old test files retain exact prefixes; the sole EP test amendment is independently
normalized to its original prefix with every assertion preserved. Final manifest:
6cce6afd4e2f288e86b9091c19d47886d89e3a92083e74a66fdbf95a23050fd4.
Initial test-only missing imports and Arc mutation errors were fixed. Root r1
Clippy found5 unique warnings; three narrowly corrected production paths passed
both independent delta reviews and affected tests. All14 root r2 gates now pass
with unchanged source and zero warnings/errors. Prior failures/freezes remain in
ignored evidence directories. Final reviewer gate confirmations precede staging.
These checks establish direct handler/store/publication behavior and one actual
queue enqueue/dequeue, not live traffic, worker replay or conformance acceptance.
R5e, R6-R8 and F1/F2 remain open; the selected original issue stays open.
Both independent reviewers confirmed final R5d SCOPED PASS against the final
manifest and all14 root gate logs. Stage the19 technical paths plus this ledger;
commit locally without push. Unrelated untracked artifacts remain excluded.

R5d committed558433cf9e9695938f77693c88d39d7b60d35917 locally, parent745850e98.
All20 staged hashes match HEAD; tracked/index state was clean after commit.
No push; concurrent untracked artifacts remain unchanged. R5e starts PLAN ONLY
for RAP URL configRefs, non-WAF Selectors and RegionRoute overrideRef. Revalidate
residual evidence on this HEAD; preserve each family's existing failure policy,
actual consumer ownership, disabled-slot behavior and immutable snapshots.

R5e source and feature authority correct the old residual inventory's blanket
Selector fallback claim: RAP denies denied/missing/disabled/wrong-type/unknown
profile selection, while Canary/GCIR fall back to base. The RAP user guide states
this explicitly; the generic ConfigData table and Canary parity comment are
stale. The plan must preserve consumer policies and correct their scoped docs,
not relax RAP under identity unification. Fresh test/caller evidence is in
ignored r5e-test-inventory-20261008.md; no implementation has started.

R5e Astra grounding identifies pre-plan gates: RAP invalid Selector must retain
an explicit failure state rather than lose its key and use default; GCIR Selector
loans must retain actual ESP ownership rather than prepared metadata alone; URL
slots validate before effective profile copying and preserve bounded warnings,
inline siblings and nested-condition second pass. Existing GCIR single-snapshot
and Region raw-overlay/proof contracts remain required. Formal plan approval is
pending; no R5e production edits or Cargo have started.

R5e independent Astra PLAN SCOPED PASS authorizes EXECUTE for31 technical paths
(14 Rust production,1 existing tracing registry,11 mirrored tests,5 docs), after
fresh original/list baseline capture at558433cf9. The existing authored-profiles
clone is permitted under actual-owner loans; invalid fields must never read,
authorize, subscribe to or copy external document candidates. No extra clone may
fabricate ownership. Original tests remain frozen; deviations require review.
Final role-specific authorization, activity, fallback, cache and warning evidence
plus two actual reviews/root gates remain pending. No next batch is activated.

R5e final candidate changes29 technical paths within the31-path allowlist.
Actual EP/ESP loans now unify RAP URL configRefs, RAP/Canary/GCIR Selectors and
RegionRoute overrideRef across identity, permission, dependencies and runtime
construction. Invalid identities stop before external access; valid missing or
denied dependencies remain recoverable. RAP Selector denial retains its existing
conditional/policy scope; Canary/GCIR/Region preserve their distinct base fallback.
No schema, queue, request-path parsing or extra owner clone is introduced.
Focused verification passes433 distinct unit tests,2 actual-owner E0505 doctests
and18 tracing-guard self-tests. All8926 old compiled test identities survive;
13 are added, and10 existing test-file prefixes remain byte-identical. The
validation-disabled child test proves that exactly one isolated test executed.
All14 root r1 gates pass with zero warnings/errors and unchanged source,
including workspace check and Clippy. Final manifest:
7882b09816d41393e885025a6c112f6feb2c6f5f0c77a0786b8f6b7abe53d4c8.
Initial test import/fixture failures and an unused test-helper warning were
corrected; original evidence remains in ignored verification directories.
These results cover direct handlers, permission transitions and runtime/snapshot
behavior, not live traffic or Gateway API conformance. R6-R8 and F1/F2 remain
open, including annotation/LinkSys/DNS/log consumers and final documentation.
Both independent reviewers confirmed final SCOPED PASS after independently
checking all14 root logs and frozen source hashes. Commit29 technical paths plus
this ledger locally without push; unrelated untracked artifacts remain excluded.

R5e committed5cd67dd91fb5e0d8e52ed30d95f61af1aa21b8ea locally, parent558433cf9.
All30 staged hashes match HEAD; tracked/index state was clean after commit.
No push. R6 starts PLAN ONLY with current-HEAD revalidation of the Service DNS
producer authorization/owner-UID observations and the complete remaining finite
annotation, LinkSys, global-plugin, template, log-output and instance inventory.
The first implementation sub-batch requires an exact allowlist and independent
plan approval. Historical async-boundary observations alone do not authorize
generic writer fences or broader ownership changes.

## Post-merge resumption (2026-10-09)

The user completed merge commit `d9289ba29` and integration-repair commit
`1764dd6d2`, then explicitly resumed the reference task. The merge-only pause
in checkpoint08 is complete. Resume serial IMPL and local commits per accepted
batch; never push. Unrelated untracked files remain outside this task.

Fresh integration evidence at `1764dd6d2`: the default fast main report has
269 PASS, 12 existing SKIP, zero failures and timeouts (281 report entries,
not individual test cases). Workspace checks and independent reviews passed.
This is a baseline, not acceptance for subsequent reference changes or live
Kubernetes/Gateway API conformance.

R6a remains plan-only pending review of its current-HEAD plan. Independent
revalidation confirms the raw Service DNS producer still bypasses the handler's
Grant gate, and generated-slice skip eligibility still omits current Service UID
and required-family identity. Prefer actual-owner borrows and existing Grant
queries; retain the explicit DNS string grammar, process-default authority,
resolver failure policies, forced SSA and last-good output behavior. R6 residuals,
R7/R8, final coverage/simplification reviews, documentation cleanup and F2 remain
open as recorded in checkpoint08.

The refreshed R6a plan at `tmp/resource-reference-contract/r6a-plan-1764dd6-20261009/`
received independent Astra SCOPED PLAN PASS for its exact 19 technical paths.
EXECUTE is authorized after baseline capture. The implementer owns the sole
production/Cargo lane; the coordinator owns this ledger. Final reviews, affected
tests, workspace gates and a local commit remain pending. Async write fencing
and obsolete-family deletion are outside this batch's approval.

R6a implementation resumed after an agent-server restart without discarding the
existing diff or completed Resources test. Two fresh independent result reviewers,
including Astra, approved the production paths and knowledge after corrections.
The reviewed narrow amendment extracts the private resolved-slice publication
step for real dual-family partial-SSA tests, still deriving ownership from the
actual Service. Both failure orders retain the previous fingerprint, retry all
desired slices and then skip a complete matching generation. The sole original
fixture amendment adds the missing Gateway name; original IDs/assertions remain.

Focused evidence so far: Resources DNS 1, Common DNS 65, Controller DNS 25 and
Controller handlers 714 pass. Fresh compiled identities are 2030 to 2031,
377 to 378 and 1798 to 1806 respectively, with no removals. An initial Controller
compile failure from an import/static-name collision was corrected; its log is
retained. Doctests, Gateway regression checks and final root gates remain pending.
Evidence is under `tmp/resource-reference-contract/r6a-verification-20261009/`.
Existing GC may delete a just-repaired mislabeled slice using its earlier list;
this remains in the separate output-authority/GC residual, not a claim of durable
single-tick repair. R6a does not close the later DNS, R6-R8 or F1/F2 work.

R6a final verification passes 848 distinct focused unit tests and one actual-owner
compile-fail doctest, including 43 unchanged Gateway DNS regressions. Both
independent reviewers confirmed the frozen 19-path source and original-test
preservation. All 14 root gates pass with unchanged source and zero warnings or
errors, including workspace check and Clippy. Gate evidence is under
`tmp/resource-reference-contract/r6a-root-verification-20261009/`. Final review
confirmation and local commit follow; no live Kubernetes/conformance claim is made.

The next read-only candidate is Gateway Service/Gateway/EGC DNS cold binding:
reuse existing discovery, GatewayInfo and EGC snapshots, preserve live typed
HTTPDNS lookup, precedence, failure outcomes and EGC raw-key winner selection.
Revalidate after the R6a commit before planning or editing. Plugin attachment,
ProxyProtocol, other fixed LinkSys consumers and the remaining R6-R8/F1/F2 work
remain separate; this candidate does not activate another mutation lane.

R6a committed `39f7870848f931b150ff75722c49d8987792467c` locally after both final
reviewers confirmed all gate logs and frozen hashes. The 20 committed paths match
the final candidate exactly; tracked/index state was clean after commit. No push.
R6b now enters PLAN ONLY at that HEAD for Gateway's three-layer DNS cold binding.
It requires current-code revalidation, an exact allowlist and independent plan
approval before implementation; no other residual family is activated.

R6b's current-HEAD 19-path plan received independent Astra SCOPED PLAN PASS;
EXECUTE is authorized. The exact plan and failure matrix are preserved under
`tmp/resource-reference-contract/r6b-plan-39f7870-20261009/`. Service candidates
publish in existing discovery generations; GatewayInfo and EGC retain their cold
snapshots. Common keeps one borrowed-layer selector, with only two supporting
Controller call adaptations. Reference-local identity validation leaves independent
inline/TTL behavior explicit, while an invalid EGC winner cannot promote another
config. Actual implementation, test preservation, performance evidence and final
reviews/gates remain open.

R6b production implementation is in place within the approved paths; both
independent incremental reviews found no semantic blocker. Tests and knowledge
are under final review. The fresh baseline lists Resources/Common/Controller/
Gateway as 2031/378/1806/5304 identities. The serial targeted runner and original
failure evidence are under `tmp/resource-reference-contract/r6b-verification-20261009/`.
New coverage uses the captured-Service production selection seam for a bounded
allocation check and the actual typed HTTPDNS store for replacement/deletion.
No whole-DNS zero-allocation, live Kubernetes or conformance claim follows.

Read-only post-DNS candidates, to revalidate after R6b commit: (C1) actual-field
binding for EGC globalPluginsRef and Gateway HTTP plugin annotations across
Controller dependency/status and Gateway publication, retaining the existing EP
Missing/Unique/Ambiguous classifier; (C2) Gateway/L4 stream annotation owners,
preserving Gateway slash rejection, L4 slash-containing grammar and fail-closed
attachment signals; (C3) Gateway ProxyProtocol resourceRef owning gate before
lookup/dependencies, retaining the existing bounded parser, visibility/Grant
ordering and resolved policy. A bounded diagnostic identity is not proof that
lookup was gated. No candidate is approved for implementation yet.

R6b targeted verification passes 315 unit executions covering 256 distinct test
IDs and two compile-fail doctests, with no failed or zero-test filters. Four crate
inventories preserve all 9519 old identities: 2031 to 2032, 378 to 379, 1806
unchanged and 5304 to 5309; ignored lists are unchanged. Six original test files
retain their bodies after the reviewed five native-call and one import adaptations
and formatting normalization. Both reviewers found no code/test/knowledge blocker.
The 19 technical paths are frozen; root workspace gates and final evidence review
remain pending. `results-summary.json` and `preservation.json` in the R6b
verification directory retain execution, deduplication and source provenance.

R6b final root verification passes all 14 gates with unchanged source. Workspace
check (87.342 seconds) and Clippy (94.589 seconds) report zero warnings/errors;
format, documentation, reference/transform contracts and the remaining guards
pass. Evidence is under `tmp/resource-reference-contract/r6b-root-verification-20261009/`.
Both independent reviewers verified the focused execution and preservation
evidence; final gate confirmation precedes local commit. No live Kubernetes or
Gateway API conformance acceptance is claimed. The next serial batch is C1
actual-field binding for EGC globalPluginsRef and Gateway HTTP annotations,
subject to current-HEAD revalidation and independent plan approval.

Both independent final reviewers, including Astra, confirmed all R6b gates and
frozen technical hashes: SCOPED PASS with no blocker. The approved 19 technical
paths plus this progress record are ready for the local-only commit.

R6b committed locally as `6bb5fa43907f7b2cebb148aacda6cc1384d44717`; all 20
committed blobs match the final reviewed snapshot. The tracked worktree and index
were clean after commit; unrelated untracked paths remain untouched. No push.
R6c C1 now enters PLAN ONLY against this HEAD for EGC global plugin fields and
Gateway HTTP plugin annotations. Stream annotations and ProxyProtocol remain
separate batches; all later R6-R8/F1/F2 residuals remain open.

R6c C1 received independent Astra SCOPED PLAN PASS at `6bb5fa439`; the exact
20-path plan is under `tmp/resource-reference-contract/r6c-plan-6bb5fa4-20261009/`.
Implementation is authorized for actual Gateway HTTP annotation and EGC global
plugin fields. Preserve Missing/invalid-target runtime drop and distinct sealed
ambiguity denial; identity validity precedes reads/edges without adding Grants.
The reviewed watch matrix validates an acquired EGC independently of plugin
watch visibility, while unavailable EGC remains Skip. Empty lexical slots create
no attachment error on their own. Stream and ProxyProtocol remain excluded.

R6c production, test quality and knowledge have two independent SCOPED PASS
reviews, including Astra. The only old fixture amendment adds a valid Gateway
name in the HTTP-annotation-specific helper; existing assertions remain intact.
New watched-store counters verify zero reads for invalid source/target identity
and exercise valid two-read controls; the EGC watch matrix includes actual
malformed owners. The first Controller run recorded 717 passes and one new
error-text assertion failure; the assertion is corrected and the raw failure
retained. Final frozen r2 verification and root gates remain pending under
`tmp/resource-reference-contract/r6c-verification-20261009/`.

R6c final targeted verification passes 790 distinct unit tests and one actual-owner
compile-fail doctest. All 9526 old compiled identities and ignored inventories
survive (9537 current). A second narrow test amendment, independently approved,
updates only the explicit-empty Gateway namespace's nonempty HTTP attachment
expectation to no keys; every original case and raw namespace assertion remains.
The earlier listener expectation failure remains recorded. Final Resources and
Controller evidence is r2, with unchanged relevant source; Gateway r3 covers the
final test amendment. Both independent reviewers verified raw logs, preservation
and the frozen 20 technical paths.

All 14 R6c root gates pass with unchanged source and zero warnings/errors;
workspace check took 82.283 seconds and Clippy 94.833 seconds. Evidence is under
`tmp/resource-reference-contract/r6c-root-verification-20261009/`. Final independent
gate confirmation precedes local-only commit. Real Kubernetes/conformance and
remaining R6-R8/F1/F2 work remain open. Next is C2 stream annotation ownership,
revalidated after commit; its differing Gateway/L4 grammar and configured failure
signals must remain intact.

Both independent final reviewers, including Astra, confirmed R6c commit readiness
with SCOPED PASS and no blocker. The approved technical paths and this record
are the complete local commit scope. No push is authorized.

R6c committed locally as `cb46155191d18f76e3e682fd2c2cfedcbf78315b`; all 21
committed blobs match the reviewed final snapshot, and tracked/index state was
clean after commit. No push. R6d C2 enters PLAN ONLY at this HEAD for actual
Gateway/TCPRoute/TLSRoute/UDPRoute stream annotation fields. Preserve distinct
Gateway slash rejection and L4 slash-retaining grammar, configured fail-closed
signals and existing permission/dependency policy. ProxyProtocol remains separate.

R6d C2's exact 24-path plan received independent Astra SCOPED PLAN PASS at
`cb4615519`; execution is authorized. The plan is preserved under
`tmp/resource-reference-contract/r6d-plan-cb46155-20261009/`. Four actual stream
attachment owners share a finite facade while retaining existing target lexical
grammars. Empty configured-invalid keys must be excluded before cold demand and
raw lookup, preserving existing unavailable/deny behavior. Review explicitly
approved the stream-specific Gateway fixture name amendment and test-local
actual-owner adapters for old detached-helper tests. Implementation, execution
and final reviews remain open.

R6d's complete code/test/knowledge checkpoint passed independent Astra static
review and enters frozen focused verification. A narrow reviewed test-plan
correction preserves the earlier backend owner gate: invalid L4 owners cannot
construct route units, so tests prove input sentinel plus unit rejection and
separately drive actual projection into existing stream admission. Shared-listener
valid/invalid attachments are tested in both key orders and under Bypass, with
valid recovery controls; Stream's own borrowed-owner doctest is included. The
full lexical/sentinel decision remains in the owning normalization knowledge page
with targeted links elsewhere. No production authority or scope was expanded.

R6d focused verification passes 850 distinct unit tests and two compile-fail
doctests. All 9537 old test identities and ignored inventories survive (9546
current); ten old test files retain their assertions under the exact approved
fixture/import/test-local adapter changes. The initial Controller test compile
failed because its new StreamPlugins fixture used an unsupported Default; only
that new fixture was corrected, and the failure log remains. Resources r1 and
Controller/Gateway r2 cover unchanged final Rust source.

Independent review approved one extra owning UDP knowledge page, bringing the
technical allowlist to 25 paths. Its original was captured before editing; final
freeze proof shows the original 24 paths unchanged. The page now describes
captured PluginScope lookup and the checker consuming the selected view, while
preserving session lifetime/revocation and Gateway UDP exclusion boundaries.
Both independent reviewers verified execution, preservation and final document
evidence under `tmp/resource-reference-contract/r6d-verification-20261009/`.

All 14 R6d root gates pass with unchanged source and zero warnings/errors;
workspace check took 89.337 seconds and Clippy 95.949 seconds. Evidence is under
`tmp/resource-reference-contract/r6d-root-verification-20261009/`. Final independent
gate confirmation precedes local commit. Next is C3 Gateway ProxyProtocol
actual-reference ownership, revalidated after commit. Other R6-R8/F1/F2 work,
including real Kubernetes/Gateway API conformance, remains open.

Both independent final reviewers, including Astra, confirmed R6d SCOPED COMMIT
PASS with no blocker after checking all gates and current frozen technical hashes.
The approved 25 technical paths plus this progress record form the commit scope.

R6d committed locally as `2944878c2fa4e372123a1e19ff406a9860c48fca`; all 26
committed blobs match the reviewed snapshot. Tracked/index state was clean,
protected unrelated untracked paths remain untouched, and nothing was pushed.
R6e C3 enters PLAN ONLY at this HEAD for Gateway ProxyProtocol resourceRef actual
field ownership. Preserve existing bounded parsing, eager Grant computation and
canonical permission decisions, recovery edges, failure/status policy and resolved
listener trust snapshots.

R6e C3's exact nine-path plan received independent Astra SCOPED PLAN PASS at
`2944878c2`; execution is authorized. The plan is under
`tmp/resource-reference-contract/r6e-plan-2944878-20261009/`. A finite Gateway
field facade owns the one canonical parse and borrows the actual owner; typed
query validity precedes diagnostic construction, dependencies and acquisition.
Parser/listener/inline precedence and eager Grant computation plus the canonical
permission decision remain unchanged. Review approved one private FnOnce lookup
seam around the existing resolver body to verify real zero/one acquisition without
a new store abstraction. Implementation and final verification remain open.

R6e received independent Astra and result-reviewer SCOPED STATIC PASS for the
complete nine-path code/test/document checkpoint. Existing Controller tests remain
an exact prefix; new tests include a real malformed-key policy decoy proving that
an explicitly empty owner namespace cannot reach acquisition. Focused verification
is now authorized with source freezing and preserved test identities/ignored cases.
Execution, workspace gates and final evidence review remain open.

R6e focused verification passes 791 distinct unit tests and two compile-fail
doctests. Independent compiler witnesses confirm both owner and parsed-field
lifetime violations are E0597, with a successful valid control. All 9546 old test
identities and ignored inventories survive (9553 current); the old Controller
test file remains an exact prefix without amendments. Both independent reviewers
confirmed execution evidence under the R6e verification directory.

The first root Clippy run found one unnecessary clone in a newly added test.
Only that assertion changed to a borrowed singleton slice; its 71 Gateway handler
tests passed again. The original failure evidence remains, and r2 preservation
proves the other eight technical paths unchanged. Fresh format, workspace check,
Clippy and diff gates now pass with zero warnings/errors and unchanged source;
the remaining nine successful r1 gates retain their unchanged inputs. Evidence is
under tmp/resource-reference-contract/r6e-root-verification-20261009/.
Final independent commit confirmation follows. R6 residual LinkSys consumers,
R7/R8 and F1/F2, including live conformance, remain open.

Both independent reviewers, including Astra, confirmed R6e SCOPED COMMIT PASS
with no remaining blocker. The nine technical paths and this progress record are
the complete local commit scope; unrelated untracked files remain excluded.

R6e committed locally as 9ee291490074312a12a5aed64f318172ab316a3e; all ten
committed blobs match the reviewed snapshot. Tracked/index state was clean,
protected unrelated untracked paths remain untouched, and nothing was pushed.
R6f enters PLAN ONLY at this HEAD to revalidate remaining fixed LinkSys consumers
and process-config provenance before choosing the next coherent implementation.

R6f read-only revalidation confirms GraphqlRateLimit.redisRef is an additional
fixed-reference source: create delegates through rate_limit_config to
RateLimitRedis::new. The later plugin-owner batch must retain this origin alongside
DSL, Redis, Webhook and AI consumers. Existing typed live-provider stores already
protect provider kind and must not be replaced with captured clients. The next
candidate is the smaller process log-output boundary: public LinkSysRef values can
bypass serde, ES/Kafka constructors do not revalidate, and factory tracing formats
those values before validation. A reviewed plan must preserve logger enablement,
fallback and synchronous SSL behavior, with no fabricated Kubernetes owner/Grant.

R6f exact 17-path plan received Astra SCOPED PLAN PASS at 9ee2914; execution
is authorized. The plan lives under tmp/resource-reference-contract/
r6f-plan-9ee2914-20261009/. It reuses a finite process-output target in refs and
makes cold key construction fallible, including direct senders and independent ES
backfill derivation. Existing SSL asymmetric outcomes and live typed provider
lookups remain unchanged. Seven old-test unwrap adaptations across four files
are explicitly reviewed. Implementation and final verification remain open.

R6f complete code/test/knowledge checkpoint received independent Astra and
result-reviewer SCOPED STATIC PASS. Seven approved unwrap adaptations preserve
old assertions; new tests cover direct senders, serde/key parity, factory branch
outcomes and pure backfill derivation without global OnceLock mutation. The 9553
baseline IDs were freshly listed from hash-verified binaries. Focused serial
verification is authorized; workspace gates and final evidence review remain open.

R6f focused verification passes 243 distinct unit tests across ten nonzero
filters with no failures. All 9553 old identities and ignored inventories survive
(9559 current). Four original test files retain all assertions with exactly the
seven approved unwrap adaptations; all 17 technical paths match the source freeze.
Both independent reviewers verified raw logs, binaries and preservation evidence.
All 14 root gates pass with unchanged source and zero warnings/errors; workspace
check took 22.499 seconds and Clippy 32.636 seconds. Evidence lives under the
R6f verification and root-verification directories in tmp/resource-reference-contract.
Final independent commit confirmation remains the last gate. Resource-owned
LinkSys consumers, process locators, R7/R8 and F1/F2 remain open.

Both independent reviewers, including Astra, confirmed R6f SCOPED COMMIT PASS.
The exact 17 technical paths plus this record are the local commit scope.

R6f committed locally as b9c18686534699fc6cfa21a7d2ba1caedca69443; all 18
committed blobs match the reviewed snapshot. Nothing was pushed. R6g begins
PLAN ONLY at this HEAD for actual HTTP plugin LinkSys references, including DSL,
RateLimitRedis, GraphqlRateLimit delegation, WebhookKeyGet and AiProxy quota.
Stream/Backend ownership, process locators and later R7/R8/F1/F2 remain open.

R6g received independent Astra and second-reviewer SCOPED PLAN PASS for eleven
technical paths at b9c1868; execution is authorized. Reuse existing PreparedEntry
finite LinkSys atoms and actual borrowed configurations for request five variants
and response DSL. This is provenance, not a new DNS target/Grant contract. Binding
and source rejection precede cache/LKG; invalid sources are non-retainable while
ordinary valid-owner DSL errors retain their existing policy. The Graphql parity
test may use its existing actual owner without changing assertions. Reuse one
prepared binding across stages; preserve masks, disabled and inline-only paths.

R6g complete static review initially requested stronger planned evidence:
internal DSL/Webhook field mutations and actual two-stage LKG publication, not
only whole-entry changes or retainability flags. These are now covered, including
quota removal, immutable/mutable stale binding rejection, nonempty cache controls,
Block precedence and sealed request execution. Both independent reviewers give
SCOPED STATIC PASS. They also approved a test-local extra None parameter and
moving the previous owner-aware request wrapper into tests; no core logic or
original assertion was duplicated/changed. Focused verification is authorized.

R6g final focused evidence passes 763 distinct unit tests plus eight doctests,
with all 9559 old IDs and ignored inventories preserved (9567 current). Compiler
witnesses confirm a valid loan plus E0505/E0597 lifetime failures. Original r1
missing lifetime, r2 incomplete new fixtures, and r3 test failures remain recorded.
Independent review approved exactly two old parity fixture names; namespace,
cases and assertions remain intact. The new LKG test now observes shared filter
Arcs, because retention recomposes the outer runtime wrapper, and executes the
invalid-source sealed 500. Resources r3 and Gateway r4 cover unchanged final
production; r3-to-r4 changed only the reviewed compile test file.

Both reviewers verified execution and preservation. All 14 R6g root gates pass
with unchanged source, zero warnings/errors, workspace check 83.395 seconds and
Clippy 91.682 seconds. Evidence is under the R6g verification/root-verification
directories in tmp/resource-reference-contract. Final independent commit review
follows. Stream/Backend LinkSys owners and other R6-R8/F1/F2 remain open.

Both independent reviewers, including Astra, confirmed R6g SCOPED COMMIT PASS.
The exact eleven technical paths plus this record form the local commit scope.

R6g committed locally as 98e585b0252984d8da090ea138a9208dfc4b2ee6; all twelve
committed blobs match the reviewed snapshot. Nothing was pushed. R6h enters
PLAN ONLY at this HEAD for remaining Stream ConnectionRateLimit and EdgionBackend
credentialPool LinkSys owner boundaries, split if their mechanisms warrant it.
No later consumer, process-locator, R7/R8 or F1/F2 acceptance is implied.

R6h read-only scope is narrowed to Stream ConnectionRateLimit. Existing actual
StreamIpListBindings owner access supports a finite owner/index loan without
extending IP topology or adding a catalogue. Preserve needs_redis activation:
unused redis_ref in local-only configuration introduces no new source gate.
Backend credentials are a separate next batch because UID/raw-key state identity,
limiter equivalence reuse and captured snapshots need independent review.
Current code also corrects historical inventory: Backend valid_redis_ref enforces
two ASCII alphanumeric/hyphen/underscore components, not merely slash shape.

R6h exact nine-path plan received Astra SCOPED PLAN PASS; execution is authorized.
A finite actual owner/ordinal loan uses immutable generation ownership without
adding IP topology or another snapshot. Check needs_redis source identity after
disabled/policy handling and before cache reuse, then consume the actual config.
Local-only unused references, failure policies, raw counter identity and live
provider lookup remain unchanged. Backend remains the separate R6i residual.

R6h received both independent SCOPED STATIC and focused EXECUTION PASS verdicts.
Final evidence covers 202 distinct unit tests and two doctests; all 9567 old IDs
and ignored flags survive (9572 current), and both old test bodies remain exact
prefixes. Valid loan compilation and E0505/E0506 witnesses establish ownership.
Initial root Clippy required an equivalent match-to-let-else rewrite; its warning
record remains. Only the runtime file changed, Gateway 188 tests passed again,
and final preservation matches nine frozen technical paths. All 14 r2 root gates
pass unchanged source with zero warnings/errors (check 14.808s, Clippy 21.019s).
Evidence lives in the R6h verification/root-verification directories. Final commit
review follows; Backend R6i and later R6-R8/F1/F2 remain open.

Both independent reviewers, including Astra, confirmed R6h SCOPED COMMIT PASS.
The nine technical paths plus this record form the exact local commit scope.

R6h committed locally as 54c0d30aaf21ce2f66f68d8b96262e093c111e8e; all ten
committed blobs match the reviewed snapshot. Nothing was pushed. R6i begins
PLAN ONLY at this HEAD for EdgionBackend credentialPool Redis provenance,
preserving current UID/raw-key state identity, limiter equivalence and snapshots.

R6i read-only revalidation confirms spec validation must precede source binding:
limits require redisRef, and unlimited pools forbid it. Preserve ASCII target
grammar, selected-credential reservation, partial-good credentials, UID/raw-key
state identity, whole-backend equivalence and pool limiter Arc reuse. Independent
inventory also flags the Controller Backend credential Secret generic resolver
as a separate actual-source review candidate: verify its complete source gate
before dependency registration during later coverage reconciliation. This does
not expand the Redis batch or establish that Secret ownership is already covered.

R6i exact seven-path plan received Astra SCOPED PLAN PASS; execution authorized.
Canonical spec validation remains first, then a finite actual Backend field loan
checks a configured Redis source before UID/credential work or equivalent reuse.
No-reference pools keep their prior behavior. Target grammar, partial-good pools,
state identity and captured snapshots stay unchanged. Secret ownership remains
separate coverage, not an accepted guarantee from this Redis-specific migration.

R6i complete code/test/knowledge checkpoint received both independent SCOPED
STATIC PASS verdicts. Tests cover schema-first errors, source/UID/raw-key cases,
None scope, actual full/partial withdrawal and recovery, captured generations,
equivalent backend/limiter reuse and configured limits in a partial-good pool.
Existing tests are unchanged. Focused serial verification is authorized; final
execution evidence, repository gates and commit review remain open.

R6i focused verification passes 143 distinct unit tests and two doctests on the
first run. All 9572 old IDs and ignored inventories remain (9578 current), and
old test bodies are unchanged. Valid loan compilation and E0505/E0506 witnesses
pass. Both reviewers verified seven frozen paths and raw execution evidence.
All 14 root gates pass with unchanged source and zero warnings/errors; workspace
check took 23.083 seconds and Clippy 32.901 seconds. Evidence lives under the
R6i verification/root-verification directories. Final independent commit review
follows; Secret ownership and other R6-R8/F1/F2 boundaries remain pending.

Both independent reviewers, including Astra, confirmed R6i SCOPED COMMIT PASS.
The seven technical paths plus this record form the exact local commit scope.

R6i committed locally as 7989ee09d46ea4e96112a0cb7251f5c015edd0ca; all eight
committed blobs match the reviewed snapshot. Nothing was pushed. Reconcile R6
remaining process DNS and Gateway locator consumers at this HEAD before choosing
the next implementation. Backend Secret-source review and output write authority
remain separate coverage items; R7/R8/F1/F2 are not closed.

Two independent current-HEAD inventories confirm Controller process DNS already
uses LinkSysKey::parse -> canonical DnsLinkSysKey; its typed HTTPDNS preparation
and kube-less system fallback need no new wrapper. Dedicated Service lookup
already requires exact Gateway reference plus UID, and Rust/Python naming agrees.
The remaining finite process-locator candidate is Gateway startup scope/UID
parsing. Preserve two nonempty slash components without trimming or DNS tightening,
UID/nonempty/scope requirements, and existing ownership/authorization boundaries.

R6j eight-path plan received Astra SCOPED PLAN PASS; execution authorized.
One borrowed process locator replaces startup and parent_matches parsing with
UID-first errors and exact existing lexical behavior. Explicitly approved:
malformed direct parent-based Route/EdgionTls projection scopes no longer match
empty/extra-slash decoys. EdgionAcme raw attachment equality remains unchanged;
this is not a claim about every projection or an authorization mechanism.

R6j received both independent SCOPED STATIC and EXECUTION PASS verdicts. All
42 distinct focused unit tests pass; all 9578 old IDs and ignored flags survive
(9582 current), and old test bodies remain unchanged. Eight technical paths match
the reviewed frozen hashes. All 14 root gates pass with unchanged source and zero
warnings/errors; workspace check took 77.467 seconds and Clippy 86.519 seconds.
Evidence lives in the R6j verification/root-verification directories. Final
independent commit review follows; Backend Secret actual-source verification,
R7/R8 and whole-task F1/F2 remain open.

Both independent reviewers, including Astra, confirmed R6j SCOPED COMMIT PASS.
The eight technical paths plus this record form the exact local commit scope.

R6j committed locally as ddd8ad65944baa5b42333c47e0364e6fb178a21b; all nine
committed blobs match the reviewed snapshot. Nothing was pushed. The next
read-only investigation reconciles remaining R6 coverage and the Backend
credential Secret source chain through processing, dependencies, grants and
lookup before approving any implementation. R7/R8/F1/F2 remain open.

R6k exact nine-path plan received independent Astra SCOPED PLAN PASS at ddd8ad6.
Execution is authorized for actual Backend ordinal Secret fields through the
existing bound acquisition pipeline. InvalidReference is a distinct internal
class, not Secret DecodeError; preserve structural status precedence, partial-good
pools and valid Missing/Denied recovery edges. Source/target-invalid references
must add no edges and perform no authorization or material reads. Existing
custom Secret aliases and native Gateway API profiles remain unchanged.

R6k production and complete test/document checkpoints received both independent
SCOPED STATIC PASS verdicts. Actual handler tests cover populated Secret decoys,
zero acquisition/dependency effects for invalid references, real successful reads,
Grant revoke/recovery, Secret delete/repair, old material removal and partial-good
status precedence. Both old test files remain unchanged prefixes. The nine
technical paths are frozen for focused execution, lifetime and schema checks;
execution evidence and commit gates remain pending.

R6k recovery retained r1 exhaustive-match and r2 new-test type-inference failures,
then r3 was interrupted by macOS dynamic-loader shutdown, not a completed test
result. The approved fixes remain within the nine paths; r4 resumes unchanged
r3 source. Two new independent Astra reviewers re-read the complete current diff
and returned SCOPED STATIC PASS. Handler lifecycle tests manually change stores
and reparse; they do not establish fresh event-to-workqueue end-to-end delivery.
F1 knowledge reconciliation also needs to verify pre-existing descriptions in
partially-invalid-not-route-exclusive.md (status emission phase) and
ai-backend-partial-pool-keeps-resolvedrefs-false.md (reason priority versus first
error selection); neither discrepancy is introduced by this batch.

R6k final focused evidence passes: 56 distinct units, 36 schema/contract integration
target tests and four doctests. Valid lifetime compilation and E0505/E0506 negative
proofs pass. All 9582 prior test IDs and ignored flags remain (9587 current), and
both old test bodies remain unchanged. Both fresh Astra reviewers verified all
35 evidence hashes, nine current technical hashes and raw results, returning
SCOPED EXECUTION PASS. All 14 root gates pass unchanged source with zero warnings
or errors; check took 43.051 seconds, Clippy 44.880 seconds. Evidence is under the
R6k verification/root-verification directories. Final commit review follows.

Both fresh independent Astra reviewers confirmed R6k SCOPED COMMIT PASS. The
nine technical paths plus this record form the exact local commit scope.

R7a exact twenty-path plan at d50005e55 received two independent Astra SCOPED
PLAN PASS verdicts; execution authorized. Actual ForwardAuth/Guardrail/AiGuard
fields must control custom Service namespace/identity and local TLS before cache
reuse and client/task creation. Reuse existing Evaluator role presence and one
captured defaults snapshot; no detached config/client pairing. Explicitly fix
Service transport to follow enabled TLS (disabled is HTTP); explicit HTTPS URLs
remain HTTPS with native trust. Shared schema tests include Webhook and internal
Edgion-auth consumers and preserve native/mixed ExternalAuth rejection. Invalid
authority must block both EntryCache and publication LKG. Internal Route R7b,
OAuth R7c and generated output R8 remain separate. Historical ExternalAuth
knowledge wording confusing internal Edgion with native support needs R7b/F1
reconciliation, not a new native support claim.

R7a complete static review initially blocked on absent wrong-object supplied TLS
coverage. The correction adds all-three-plugin no-Tokio preparation, populated
EntryCache and real PluginStore mismatch cases, with the exact identity-mismatch
error asserted before PEM parsing can mask a missing identity gate. Both Astra
reviewers now return FULL SCOPED STATIC PASS. Approved verification clarification:
cold Service preparation does not read Service/EndpointSlice stores; prove actual
namespace/key/mode, unchanged pinned resolver behavior and source ordering, not a
fabricated read counter. No-Tokio proves no health task spawn, not a direct client
construction count. The planned 9587-ID preservation and focused execution follow.

During R7a verification the user explicitly reported pushing the existing branch.
This was a user operation; agent authority remains local batch commits only,
with no agent push or automatic pull. Local implementation baseline is unchanged.

R7a focused execution passes 832 distinct units, 28 CRD tests and 12 doctests.
Valid lifetime compilation and E0505/E0506 negative proofs pass. All 9587 prior
test IDs and ignored flags survive (9600 current); original test bodies retain
only the three approved constructor adaptations. Both Astra reviewers verified
61 evidence hashes, all twenty technical hashes and raw logs, returning SCOPED
EXECUTION PASS. Retained failures cover the shared-accessor immutable reborrow
and new-fixture imports, required fields and correct async companion observation;
production stayed unchanged after the reborrow correction. All 14 root gates pass
with unchanged source and zero warnings/errors; check took 76.057 seconds and
Clippy 84.558 seconds. Evidence is under the R7a verification/root-verification
directories. Final commit review follows; R7b/R7c/R8/F1/F2 remain open.

Both independent Astra reviewers confirmed R7a SCOPED COMMIT PASS. The twenty
technical paths plus this record form the exact local commit scope.

R7a committed locally as 418e353ddd2e7a54ddf621c1fbaa4da5a4e7e7a8; all twenty-one
committed blobs match the final reviewed snapshot. No agent push. R7b starts
PLAN ONLY for the internal HTTPRoute Edgion-auth field, with declaring Route,
delegation, backend-filter provenance and separate root wakeup preserved. Native
ExternalAuth support is not authorized by this migration. Baseline is 9600 IDs.

R7b exact thirty-path plan at 418e353dd received two independent Astra SCOPED
PLAN PASS verdicts; execution authorized with a 9600-ID baseline. Use actual
declaring Route tokens for readonly material acquisition and independently
validated root receipts for recovery, preserving pipeline owner equality. Raw
root/child sidecars and material are sanitized before early exits/inheritance;
valid pending references register recovery before reads or unrelated rule drops.
Effective Gateway locality grants no child authorization capability. The shared
TLS identity projection retains actual loans and all pairing/inheritance checks.
A default delegation callback must yield sanitized fail-closed auth decisions,
never preserve raw material through an empty result. Four old filter literals in
two approved fixture files may gain None; other fixture changes require review.
Production checkpoints and full-static reviews precede focused execution.

F2 read-only preflight during R7b confirms Docker linux/aarch64 and local kind,
kubectl and Go tools. kind lists edgion-standard-162, but the current kubectl
context is orbstack (other named context eruie2e-655cd051). Future conformance must
select its owned test cluster/config explicitly and use newly built binaries;
no current-context mutation or acceptance test ran in this preflight. System Go
is 1.24.0; verify the conformance module/toolchain before running it.

R7b early Resources review blocked an effective-field hybrid OwnerSnapshot built
from cloned root metadata and child locality. Downstream work paused. Independent
Astra approved the in-scope correction: extract the existing namespace-only target
classifier, retain authored owner validation, and use a private target-only TLS
projection borrowing the actual Route/config without token/Grant/freeze authority.
Authored preparation and bind both validate the actual managed target before
material tokens; direct-root sidecars must match actual semantic namespace, child
locality must be valid, and unrelated-filter sidecars are rejected. Preserve prior
error precedence rather than weakening old assertions. Correction implementation
resumes; this is not a completed production or execution verdict.

R7b complete fifteen-production-path checkpoint received two independent Astra
production PASS verdicts. Review confirmed duplicate root auth acquisition and
a second child-rule clone. A scoped consolidation was independently approved:
validate source first, prepare root decisions once before delegation selection,
return private decisions with existing no-delegation behavior, apply them to
authored output in the handler, and let readonly status consume diagnostics only.
Child mirrors reuse the sanitized rule clone. Default callbacks stay fail-closed.
An affected-library compiler preflight is authorized after this consolidation;
it is not final test acceptance. Full tests/docs, frozen execution and final
reviews remain required.

R7b affected-library compiler preflight passed in 76.55 seconds, with all fifteen
production hashes unchanged from checkpoint 2. The complete thirty-path static
snapshot preserved old test bodies except the four approved None additions.
Both independent Astra reviewers accepted the production call chain but blocked
full-static acceptance on missing planned Controller tests: ConfigMap CA material
and recovery, disabling TLS cleanup, child authorization for root-local targets,
and recovery dependencies retained after independent rule drops. The fresh
reviewer additionally requires exact Gateway target variant/mode assertions,
nonempty global TLS snapshot capture/inheritance, and real unresolved-template
sealing. Only allowed test files are authorized for these additions; production
remains frozen. Focused execution, lifetime proofs, workspace gates and final
commit reviews are still pending. The original static snapshot is retained.

Read-only R7c inventory confirms the remaining OAuth2 transport seam: Controller
resolution clones CredentialSourceConfig into runtime state, then OAuth2Provider
constructs a client from detached TLS config. Existing source_is_current already
checks registry/raw LinkSys identity, frozen references, exact Grant revision and
Secret UID/resourceVersion around issuer, fallback and publication, with existing
Lease/generation fences. Reuse these protections; do not redesign the protocol.
A candidate should retain the actual resolved LinkSys owner through the canonical
Common TLS/client builder. Webhook already retains its owner/binding and validates
Service kind canonically; detached target reconstruction alone is not evidence
for another migration batch. Exact R7c scope remains subject to plan review.

F1 read-only inventory retains concrete closure items: common/mod.rs still
incorrectly attributes the literal core alias to Gateway API; correct the claim
without blindly deleting the helper or changing native/custom profiles. Update
EN/ZH CredentialInjector group defaults/extension wording, retain already strict
ServiceRef/RequestMirror documentation, reconcile old manifest pending rows, and
correct the recorded Backend status-owner and first-error knowledge drifts.
Original issue deletion and inbound-link cleanup remain gated on F2 and repeated
whole-task review. No F1 production or user-document edits have started.

R7b r2 closes six static coverage gaps with unchanged production and preserved
old tests. The real unresolved-template test exposed a production gap: outbound
request rendering uses an empty-value policy for missing Secret map entries.
Both Astra reviewers approved a minimal correction in PreparedRouteAuth::new:
use the existing collect_secret_ref_names walker and require every exact key in
the same actual config's resolved map before TLS/client/health work. Existing
empty values remain valid; intrinsic template syntax validation remains intact.
Use a fixed redacted error, no new parser or authorization capability, and cover
missing map/key, decoy keys, populated/empty values and no-Tokio health failure.
The r2 snapshot is retained; corrected r3 review and execution remain pending.

R7b corrected r3 changes only the approved client preparation and two new-test
files relative to r2, adding exact missing-key rejection and valid empty-value
controls. Full-static re-review is pending. Adjacent read-only audit confirms the
same missing-map cold-check gap in PreparedManagedHttp for ForwardAuth, Guardrail
and external AiGuard: intrinsic validation and request compilation do not inspect
resolved map keys. Follow up with one private canonical outbound presence helper
shared by Route and managed preparation before EntryCache/client construction;
cover populated-cache/PluginStore replacement without LKG masking, empty values,
and disabled/inline behavior. Do not claim observed unauthorized traffic. This
follow-up is required before whole-task closure, outside the frozen R7b batch.

Both Astra reviewers returned R7b r3 FULL SCOPED STATIC PASS. Root independently
verified all thirty hashes and diff whitespace. Serial focused/lifetime and
preservation execution is authorized with per-stage HEAD/source freeze checks;
no test pass or final gate is inferred from static review.

The first Resources test build exposed a missed test-only source() consumer in
refs/outbound_tls_test.rs (E0599); its failed log is retained. Both independent
Astra reviewers approved a thirty-first technical path with one exact helper
adaptation: observe the same original token through the private Authored origin,
retain its source-kind assertion, and reject Effective in this authored helper.
All target/material/count assertions and test IDs remain; no production API or
visibility is widened. Preservation evidence must name this additional exception.
Re-freeze and rerun Resources before continuing serial focused verification.

R7b execution: Resources refs 123, Resources doctests 54 and Common TLS 49 pass.
A missing RawStore import in a new Controller test was mechanically corrected;
its failed compile log is retained. Controller auth five tests pass. The HTTP
suite then reported 67 passes and one new queue-test failure: the fixture's
is_empty() drain ignores scheduled 100ms requeues. Independent diagnosis approved
ordinary Tokio plus bounded real dequeue-await until each generation is quiet,
retaining original root-wakeup/material/cleanup assertions. A proposed paused
clock was withdrawn after checking the queue's std::time::Instant deadlines.
No production queue changes, manual root enqueues or fixed sleeps are permitted.
Freeze and rerun the affected suite before acceptance.

R7b Controller execution now passes auth 5, HTTPRoute 68, delegation 47 and
request templates 9. Gateway's new global snapshot fixture initially used a SAN
policy unsupported by the existing HTTP transport; independent review approved
using observable supported verify:false with the real provider capability while
retaining capture, next-generation rejection and old-generation construction
assertions. No production transport policy changed. Gateway client 36, pinned
resolver 4, target 1, ForwardAuth 10, runtime 445, request filter 29, and CRD 28
now pass. Failed compile/fixture runs remain recorded. Independent lifetime and
9600 prior test-ID/ignored preservation checks are next, before root gates.

Both independent Astra execution reviews pass: 826 distinct units, 28 CRD tests
and 54 doctests (908 total), with 9600 old IDs preserved and 21 added. Root gates
pass except the warning-free Clippy requirement: a new six-element target tuple
triggers type_complexity (one diagnostic plus duplicate summaries). Independent
review approved a private same-module type alias preserving element order, Eq,
construction and visibility. Re-run Resources refs/doctests/lifetime, retain
honest semantic-equivalence reuse evidence for other suites, then rerun root
gates on the final source. No commit is authorized by a warning-bearing gate.

R7b r8 received two independent corrected EXECUTION PASS verdicts. Both verified
the exact alias equivalence and fresh Resources/lifetime evidence. Final root r2
passes all fourteen gates with unchanged source and zero warnings/errors:
workspace check 21.291 seconds, Clippy 31.533 seconds. The final scope is thirty-one
technical files plus this ledger; protected unrelated untracked paths remain
excluded. Final commit review is next. R7a template-presence follow-up, R7c, R8,
F1/F2 and original issue closure remain open; no agent push.

R7b committed locally as 78d0f168cf039e060a93a23455ffe08d1549c404 after both Astra
SCOPED COMMIT PASS verdicts. All thirty-two committed blobs match the reviewed
manifest exactly; no agent push. Next batch is plan-only for the adjacent managed
HTTP missing-template-material gap and shared private presence check, preserving
actual owner, cache failure policy and legitimate empty Secret values. Baseline
is 9621 test identities. R7c/R8 and complete F1/F2 closure remain pending.

Read-only R8 revalidation identifies concrete Gateway TLS invalid-target edge
ordering gaps: backend clientCertificateRef, listener certificateRefs and frontend
CA registration can precede group/kind validation. Preserve ordered serving-cert
partial-good outcomes and frontend StrictReject in an actual-field migration.
Separate BTP/EBTP material bindings must preserve already migrated policy targets,
BTP local-CA rules and custom health same-namespace/inactive-protocol behavior.
EdgionTls already gates group/kind; audit actual source/name before its raw Secret
lookup rather than redo policy. ACME already has strict targets and UID/generation,
LeaderFence and target UID/resourceVersion checks; generated-output binding must
reuse them without new policy. ACME knowledge's stale alias-issue-absent claim is
another F1 documentation correction. These are candidate boundaries, not accepted
implementation plans or completed coverage.

The R7a-template follow-up at 78d0f168c received two independent Astra PLAN PASS
verdicts for seven technical paths: two production, two tests and three existing
documents. Extract one private canonical map-presence predicate for actual Route
and managed loans; preserve empty values, inactive entries and existing cache/
publication semantics. The fixed diagnostic broadens TLS material to HTTP
configuration because missing template keys are not TLS errors. Correct focused
filters to ai::guards::semantic and ai::guards::request (the public HTTP names
are re-export facades), require nonzero tests, and exercise templated method only
through ForwardAuth while retaining fixed POST for Guardrail/AiGuard. Execution
is authorized; no technical completion or fresh test result is claimed yet.

R7a-template full-static snapshot r1 received two independent Astra PASS verdicts.
Seven hashes match, both existing test files retain exact HEAD bodies, and four
new tests cover actual three-owner preparation, all canonical template surfaces,
no-Tokio health ordering, cache companions, real publication replacement/repair/
delete, captured Arcs and inactive/inline behavior. Serial focused verification
and identity preservation are authorized; final workspace gates remain pending.

R7a-template focused execution passes 638 distinct Gateway unit tests across
seven nonempty suites, with no failed run. All 9621 old IDs survive (9625 current),
ignored sets and old test bodies are unchanged. Both Astra execution reviews
independently verified source/log/binary hashes and assertions. All fourteen root
gates pass with unchanged source and zero warnings/errors; workspace check took
15.555 seconds and Clippy 22.781 seconds. Evidence lives under the r7a-template
verification/root-verification directories. Final commit scope is seven technical
files plus this ledger. OAuth2 R7c, R8, F1/F2 and original issue closure remain
pending; no agent push.

R7a-template committed locally as e954a9881288896c88643b08007efe3c687d1de1 after
both independent Astra SCOPED COMMIT PASS verdicts. All eight committed blobs
match the reviewed manifest. The known adjacent three-plugin missing-map gap is
closed through the shared actual-owner cold predicate. No agent push. R7c OAuth2
actual resolved LinkSys capture enters plan-only at baseline 9625; existing async
Grant/source/Secret/Lease/CAS fencing must remain intact. R8/F1/F2 remain open.

R7c eleven-path plan at e954a9881 received two independent Astra PLAN PASS
verdicts. Controller captures a real scrubbed LinkSys before moving this reconcile's
resolved TLS into a private Arc owner; returned wire stays scrubbed. Existing
PreparedLinkSysTls captures owner/defaults before DNS await and the OAuth2 factory
consumes the same proof/config through Common. Preserve all source/Grant/Secret,
issuer/fallback/publication, Lease/CAS and error-classification fences. Only two
old fixture construction helpers may adapt. No-Serialize/Debug negative compile
proofs require accessible positive controls. Baseline is 9625 identities; production
checkpoint review precedes an authorized compiler preflight and full verification.

R7c four-production checkpoint received two independent PASS verdicts; Controller
lib compiler preflight then passed without warnings or source changes. Complete
r3 static review passes after adding the planned actual System-root OAuth client
positive control and one mechanical test import for CredentialSourceProvider.
The two old construction helpers and that explicit import are the only approved
old-test adaptations; all assertions/IDs remain. Five appended tests cover actual
private owner/material and wire separation, Grant/missing/repair/disabled states,
exact TLS pairing and default/owner capture across await. Serial focused suites
and accessible-positive Debug/Serialize negative compilation are authorized.

R7c focused execution passes 102 distinct Controller tests (LinkSys 34, credential
manager 54, CredentialInjector 14); 9625 prior IDs are preserved with five new IDs.
A new-test error-conversion compile failure was corrected mechanically and kept
in the evidence. Root's fourteen gates pass with unchanged source, zero warnings
and errors (check 19.521 seconds, Clippy 24.457 seconds). Independent review blocked
only the Serialize negative proof: selected serde metadata used a different
artifact identity, so String alone was an insufficient positive control. Replace
that proof with a same-dependency known-Serialize positive control through the
actual wrapper Deref target, then require the wrapper itself to fail E0277 without
artifact mismatch or visibility errors. Preserve the original invalid proof logs;
no production change or rerun of the 102 passing tests is needed.

R7c corrected trait proof r2 received two independent Astra EXECUTION PASS
verdicts. The actual Snapshot Deref target is Serialize with the exact same
Controller/serde artifact pair; Snapshot Debug and Serialize each fail E0277
without visibility or dependency-version errors. Invalid earlier attempts remain
recorded and excluded. Eleven technical source hashes remain at r4. All 102 tests,
identity preservation and fourteen root gates stand. Final commit scope is those
eleven files plus this ledger; R8/F1/F2 and original issue closure remain open.

R7c committed locally as df08e402b6c43e0b2f10e7e6cbdadf4bef0c64e7 after two
independent Astra SCOPED COMMIT PASS verdicts. All twelve committed blobs match
the final manifest; no push. R8a Gateway TLS enters plan-only: validate actual
native declaring fields before dependency creation, preserve valid denied/missing
recovery registration before Grant reads, and reuse canonical material acquisition.
Serving-certificate partial-good outcomes and frontend StrictReject are invariant.

R8a first plan review requires amendments before execution: native ObjectName
length must count Unicode characters consistently with its schema; Gateway TLS
matching must use the same omitted-namespace default as Controller outcomes.
Controller preserves ordered successful and failed certificate outcomes, but the
Gateway continues to require a complete valid serving-certificate set. Both
reviewers verified that distinction; subset serving is not authorized. The user
added unrelated commit a85aed2fb during planning. Rebase the plan evidence on that
HEAD and capture its new test identities before mutation; 9630 remains the prior
batch count. No technical edit or Cargo execution has started for R8a.

R8a revised fifteen-path plan r2 received two independent Astra PLAN PASS
verdicts at a85aed2fb. The amendments explicitly align native character-count
validation and the existing matcher default namespace, without changing schemas,
explicit-empty semantics, attachment proofs or physical listener keys. First
capture the current-HEAD test baseline, then implement within the frozen scope.
Production checkpoint reviews precede compile preflight; focused execution and
final workspace/commit gates remain required. This is plan approval only.

R8a current-HEAD baseline is 9637 identities across the four tracked production
crates, including seven user-added Gateway tests. Gateway compilation/listing
exited zero in 176.937 seconds and listed 5363 tests; this was inventory, not test
execution. Exact binary/source hashes and original files are preserved under
tmp/resource-reference-contract/r8a-baseline-20261009. Approved production work
has started; no implementation acceptance is claimed.

R8a production r3 received two independent Astra PASS verdicts. Review rejected
r1's frontend cloning before CA limits and r2's separate pre-parse validation
loop. Both validate and parse now use one bounded frontend validator; malformed
projection remains StrictReject without oversized CA traversal or cloning. Seven
production hashes are frozen. Affected-library compiler preflight is authorized,
followed by approved tests/docs and full static review; no test pass is claimed.

R8a affected-library compiler preflight passes in 19.26 seconds with zero
warnings and unchanged HEAD/seven production hashes. Approved new tests and
four documentation updates are in progress; no focused execution yet. Evidence:
tmp/resource-reference-contract/r8a-verification-20261009/compiler-preflight-r3.

R8a full-static r1 is blocked before test execution. Two new invalid matrices
need typed-field mutation after valid deserialization; the DER dedup control
must cross the aggregate certificate threshold; owner/target namespace wording
needs correction. Root and implementer additionally confirmed a serving regression:
FS/etcd retain omitted Secret namespace as a bare store key, while the new native
lookup reads only default/name. Frontend recovery also ignores bare source events.
Prepare and independently review a bounded nineteen-path amendment covering
authorization-preserving lookup, ambiguity rejection, unchanged raw material
metadata and typed event recovery. No expanded production edit is authorized yet;
all current technical sources remain frozen. This blocker cannot be deferred to
closure while claiming R8a complete.

## R8a final verification and local commit gate (2026-10-09)

Actual Gateway backend, serving and frontend CA fields now use the finite borrowed
reference facade and bound acquisition. Validation precedes dependency/Grant/store
effects; valid missing or denied references retain recovery. Native defaults and
required frontend fields remain exact, with no native `core` alias. Frontend bounds,
complete serving sets, strict rejection and controller Grant-disable policy are
preserved. Gateway-only default material selection distinguishes missing, unique
and ambiguous bare/qualified candidates, retains actual metadata, and uses actual
source events for typed CA and ordinary Secret recovery. Non-Gateway bound/frozen
consumers retain exact reads. No new handler IO, worker or data-plane tracing.

Both Astra reviewers approved the plan amendments, production checkpoint and final
static/execution results. Discriminating tests separately cover ordinary-only
Gateway recovery, typed Secret/ConfigMap isolation and aggregate CA DER deduplication.
Final evidence: `tmp/resource-reference-contract/r8a-verification-20261009/verification-final-r7.json`.
There are 300 distinct unit and 55 doc passes, ten existing doc ignored cases, and
9637 preserved baseline identities with nineteen additions. Two approved old-test
adaptations add a valid Gateway fixture name and align raw-None/default material
acceptance while retaining explicit-empty/nondefault rejection controls. All other
old test bodies and ignored states are preserved. Lifetime positive and E0505,
E0506 and E0502 negative controls use a verified actual Resources artifact.

Raw failed runs are retained: new observer-path compilation, the superseded old
namespace assertion, and root formatting/lint/doc gates. Formatting-only source
equivalence is proven explicitly; changed test scopes were rerun as required.
Four new-test Clippy warnings were fixed without suppression. A separately reviewed
WAF index broken link was changed to a truthful historical-path statement only.
All fourteen final root gates passed without warnings; workspace check took
15.226 seconds and Clippy 18.281 seconds. Root evidence is under
`tmp/resource-reference-contract/r8a-root-verification-20261009/`.

No live TLS handshake or Gateway API conformance is claimed. R8b BTP material
pairing, later R8 consumers, F1 whole-task audit/cleanup and F2 fresh live validation
remain open. Only the frozen twenty paths and this ledger may enter this batch's
local commit. Protected unrelated untracked paths remain excluded. Never push.

## R8b planning resumed after R8a commit

R8a committed as 67ae8580baafde977bc518773b05a7bb39e85893. Its final committed
blob proof is stored in the R8a root verification directory. Post-commit status
contained only the two protected unrelated untracked paths.

R8b is plan-only: revalidate actual BackendTLSPolicy CA/client-option ownership
and typed material pairing from this HEAD. Existing canonical targetRefs, native
group and same-namespace contracts, custom option trim/DNS grammar, Gateway policy
winner capture and pool rotation must remain intact. Omitted-namespace FS/etcd
material and source-event recovery require explicit coverage. EBTP remains a
separate later batch. No R8b implementation or conformance acceptance is recorded.

R8b plan r2 received two Astra approvals. The exact twenty-two-path plan and
source baseline are under `tmp/resource-reference-contract/r8b-plan-67ae8580-20261009/`.
It requires the native eight-CA-reference bound before preparation, validation,
acquisition or material matching; partial status and complete publication check
material cardinality before scanning. Existing duplicate-reference rejection is
an Edgion rule, not a demonstrated CRD uniqueness requirement; F1 must retain that
distinction. Production implementation is authorized, with a frozen production
checkpoint and independent review required before compiler preflight.

## R8b final execution and commit gate (2026-10-10)

BackendTLSPolicy CA ordinals and the custom client option now borrow their actual
policy owner. Shared material identity/provenance checks prevent same-name Secret
and ConfigMap substitution. Common retains its original strict namespace and
error-priority behavior. The native eight-reference bound and partial/full
material cardinality checks precede preparation and matching. Controller status
and Gateway cold publication share the actual pairing contract; peer construction,
policy precedence, immutable captured generations and pool identity remain intact.

The finite default candidate selector and source-event notification scope extend
to BTP without extending Gateway-only disabled-Grant behavior or frozen authority.
Raw material metadata and raw dependency observers are retained; ambiguous default
candidates fail closed. The two cached reads are not atomic: registered dependencies
and subsequent events provide reconciliation.

Two Astra reviewers approved plan, production, complete tests and execution. After
turn interruption, two fresh Astra reviewers independently rechecked the actual
diff and final correction evidence. Accepted focused evidence contains 280 distinct
unit and 58 doc passes, ten existing ignored docs, 9656 preserved baseline IDs and
fifteen additions. Actual binaries and lifetime artifacts are hashed; accessible
lifetime positive and E0505/E0506/E0502 controls pass. Only the reviewed eight store
fixture substitutions and two conf-handler fixture fields alter old test bodies;
all old assertions and IDs remain. Raw failed runs are retained (new unordered
dependency assertion, old trustless fixture, and one new-test Clippy warning).

The final Clippy correction changes only a new-test string conversion; its eleven
Secret-handler tests passed again without warning and Controller identity/ignored
lists stayed identical. Source-input scoped reuse is explicit across revisions.
Evidence: `tmp/resource-reference-contract/r8b-verification-20261010/verification-final.json`
and `clippy-correction-r6.json` in that directory; final technical snapshot is
`tmp/resource-reference-contract/r8b-plan-67ae8580-20261009/full-static-snapshot-r6.json`.

All fourteen root checks pass without warnings. Following the single test-only
correction, six affected checks were refreshed and eight unchanged-scope checks
were reused with recorded rationale. Final workspace check took 9.686 seconds and
Clippy 11.777 seconds. Evidence and final commit manifest are under
`tmp/resource-reference-contract/r8b-root-verification-20261010/`.

Remaining work: EBTP encrypted-health TLS, EdgionTls actual references/material,
ACME/generated identities, F1 full consumer/simplification/knowledge cleanup, and
F2 fresh admission, dynamic integration and split-profile Gateway API conformance.
The existing duplicate BTP CA declaration rejection is an Edgion behavior, not a
vendor-CRD uniqueness requirement; retain that distinction in F1. The now-unused
BTP client_certificate_secret_ref wrapper is an F1 inventory candidate, not
authorized for opportunistic deletion here. Original issue closure remains open.
Only the frozen twenty-three paths plus this ledger enter the local commit.
Protected unrelated untracked paths remain excluded; never push.
