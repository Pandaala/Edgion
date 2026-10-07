# Execution and closure ledger

## Authority and reporting

The user now requires root coordination, delegated IMPL implementation and a local
commit for the current verified checkpoint and every subsequent completed batch.
Never push. Keep exactly one mutation lane; read-only inventory may overlap.
Each implementer reads original rules and knowledge, returns a plan before edits,
obtains independent Astra plan approval, implements and verifies, and obtains a
fresh independent Astra result review. Material deviations require renewed plan
review. Root checks the diff, gates, staged scope and commit result.

Checkpoint reviewer capacity note: creation briefly returned `agent thread limit
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
- The native five-Route backend/Grant plan has independent Astra PASS; its
  implementation has not started. The first checkpoint audit and workspace
  commit gates are in progress. The user committed the accumulated code snapshot
  as 0f40e2708496e9aacafb0dc2fbd2cdd786ffa9e8 (`tmp commit`) while verification
  was running. This workflow has not made a commit yet and will not amend or
  reset the user's commit. It will commit the verified remaining documentation
  and execution records before starting the next batch.
- HEAD advanced externally to 4dbcc5495. Revalidate scope against current HEAD;
  retain unrelated changes and do not stage unverified artifacts indiscriminately.

## Current checkpoint gate

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
28/eight and three-test evidence. The coordinator's closing commit is ready.
The later native Route slice has not begun, and full-manifest/conformance closure
remains open.

## Serial batch queue

The eight remaining implementation batches are provisional boundaries. Before
each batch, inspect current code and freeze exact files, manifest rows, tests and
acceptance criteria. Split a batch when independent ownership or complexity
requires it; do not silently expand an approved allowlist.

| Batch | Required result and critical acceptance | State | Commit |
| --- | --- | --- | --- |
| C0: current checkpoint | Independent accumulated-diff audit, resolve immediate introduced blockers, run required pre-commit gates, stage only verified task changes; describe partial completion honestly | scoped final PASS; all gates passed; ready for closing commit | user snapshot 0f40e2708; closing hash recorded in the next batch |
| R1: native Route backends and delegation | Actual five-Route owners; authored terminal loans alone mint Grant; invalid descriptors cause no lookup/edge; child authorizes delegated targets, root owns wakeup; dependencies precede Grant reads; preserve slots/weights, AI facts and coalesced recovery | plan approved, implementation pending; exact scope in 04 | pending |
| R2: final Gateway backend selection | Consume cold canonical targets through HTTP/GRPC/L4 final dial; retain weighted invalid slots and denial behavior; no new request allocations, locks or duplicate classifier; preserve direct endpoint/retry semantics | pending | pending |
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
