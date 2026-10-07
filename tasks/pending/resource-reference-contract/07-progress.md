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
and fresh result review roles. The current fresh result reviewer's runtime model
is unknown and its outcome must be described as independent review, never as an
Astra PASS. The confirmed Astra checkpoint audit/plan review and required later
multiple confirmed Astra whole-task reviews remain distinct gates.

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
  The first checkpoint audit and workspace commit gates passed. The user
  committed the accumulated code snapshot
  as 0f40e2708496e9aacafb0dc2fbd2cdd786ffa9e8 (`tmp commit`) while verification
  was running. The coordinator completed C0 in local closing commit
  8dab81bc69cd33094e12962809bb3c0cca7e1c19, including the final test correction,
  documentation and execution records. No push occurred. The user's commit
  remains unchanged.
- The earlier external baseline was 4dbcc5495; current HEAD is the completed
  R1 commit 0fea0a618. Revalidate each batch against its current HEAD.
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
plan must explicitly reconcile historical existence-first status with the
approved authorize-before-discovery contract and current vendored semantics.
Neither inventory authorizes implementation; revalidate overlapping files after
the R1 commit.

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

## Serial batch queue

The eight remaining implementation batches are provisional boundaries. Before
each batch, inspect current code and freeze exact files, manifest rows, tests and
acceptance criteria. Split a batch when independent ownership or complexity
requires it; do not silently expand an approved allowlist.

| Batch | Required result and critical acceptance | State | Commit |
| --- | --- | --- | --- |
| C0: current checkpoint | Independent accumulated-diff audit, resolve immediate introduced blockers, run required pre-commit gates, stage only verified task changes; describe partial completion honestly | complete; scoped final PASS and all gates passed | user snapshot 0f40e2708; closing 8dab81bc6 |
| R1: native Route backends and delegation | Actual five-Route owners; authored terminal loans alone mint Grant; invalid descriptors cause no lookup/edge; child authorizes delegated targets, root owns wakeup; dependencies precede Grant reads; preserve slots/weights, AI facts and coalesced recovery | complete; fresh independent Astra scoped PASS and all gates passed | 0fea0a618 |
| R2: final Gateway backend selection | Consume cold canonical targets through HTTP/GRPC/L4 final dial; retain weighted invalid slots and denial behavior; no new request allocations, locks or duplicate classifier; preserve direct endpoint/retry semantics | R2a complete; fresh independent Astra scoped PASS and all gates passed; R2b/R2c pending | local commit gate; hash recorded after commit |
| R3: filters, ExtensionRef and mirror | Actual HTTP/GRPC filter owners and child namespaces; authored/resolved coverage; correct runtime compilation, local descriptor ownership and recovery; mirror denial affects only mirror with role-qualified dependencies | pending | pending |
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
