# Execution and closure ledger

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
