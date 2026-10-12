# ARCH-002 request-body contract ownership

## Meta

| Key | Value |
| --- | --- |
| Created | 2026-10-11 |
| Status | completed: independently approved result integrated into current local branch |
| Type | refactor |
| Priority | P3 |
| Workflow | AIMPL-C / AIMPL-CONFIRM |
| Selected finding | ARCH-002 |
| Anchor | edgion-gateway |

## AI guide summary

ARCH-002 is implemented: the four existing request-body read contracts now belong
to `request_body`, and actual consumers use that canonical owner. User approved V1
before design and implementation. Independent design and final result reviews
passed; workspace check/Clippy, project guards and 275 focused tests passed.
Runtime behavior remains unchanged. The user subsequently authorized transferring
the reviewed result into the original worktree's current local branch.

## Selection and current evidence

Input: `tasks/review-architecture-20261009/ARCH-002-request-body-contract-ownership.md`.
Read its independent receipt `VERIFICATION-ARCH-002.json` as historical evidence;
current source and rule hashes differ from parts of that receipt.

`plugins/runtime/traits/session.rs` declares `BodyReadError` at line 37,
`BodyReadResult` at 97, `RequestBodyChunkConsumer` at 102, and
`RequestBodySnapshotReader` at 124. `request_body/storage.rs`, `state.rs`,
`state/enabled.rs`, `operation.rs` and `body_scanners.rs` import contracts from
that consumer module. Session imports body-owned operation errors and results.

The existing WAF resident-slice capability is a concrete maintenance example:
storage implements `try_take_contiguous_memory`, while its definition lives in
plugin session. `session_adapter.rs:248` performs bounded snapshot consumption
with real EOF/parity checks. `request_mirror/plugin.rs:49` owns a detached reader
and isolation budget without retaining PluginSession. The old
`tasks/pending/waf-optimization-design/01-spec.md` no longer exists; current
implementation, rather than that historical task, supports the assessment.

## Candidate proposal V1

Make `request_body` the canonical owner/exporter of exactly those four unchanged
contracts. Migrate actual imports and qualified references in Gateway production
code and unit tests, and update directly relevant architecture, contributor and
semantic knowledge material after approval. PluginSession retains policy, stage
and transport methods and consumes these body-owned contracts.

The benefit is one discoverable owner for snapshot/source/read invariants beside
storage and operations; future storage capability changes no longer require
placing their definition in the common plugin-session contract. Consumers still
need updates when a capability changes. No throughput, memory, compilation-time
or measured engineering-time improvement is claimed.

Keep the existing design as the simpler alternative if this modest ownership
benefit does not justify import migration. Splitting a file inside plugin runtime
alone would not correct dependency direction.

## Boundaries and risks

- Exclude ARCH-001 and ARCH-029, new body APIs, feature capability redesign, WAF
  implementation, dependency changes, other crates and unrelated cleanup.
- Retain `RequestBodyInspectionCapture` and the observer's PluginLog dependency;
  this does not claim complete independence of the body subsystem from plugins.
- Preserve error variants/display/policies, async_trait signatures/defaults,
  immutable snapshot generations, replay cursors, proven-empty versus uncaptured
  state, isolation budgets/backing lifetime, real EOF/parity, physical/logical
  admission, prefix/stage gates, retries, cancellation and storage deadlines.
- Add no buffer, request copy, allocation, lock, cache, dispatch or compatibility
  re-export. Preserve canonical request-only BodyStorage, logging ownership,
  access-log/redaction rules, observability restraint and the workspace DAG.
- Migration risks are stale explicit/qualified imports, test-only/mockall scope,
  async trait-object wiring and rustdoc links. Source reasoning suggests an
  unchanged declaration move is feasible; execution verification remains pending.
- Current symbol references occur in 22 production and 26 unit-test files;
  references do not imply all these files need edits. No matches were found in
  Gateway benchmarks or edgion-tests, so there is no assumed benchmark migration.

## Acceptance and next gates

One definition per contract under request_body; body storage/state/operations/
scanners obtain no fundamental body contract from plugin runtime; real consumers
use the canonical owner, with no legacy public re-export. Preserve protected unit
test identities and layout. Verify existing snapshot/state/storage, adapter
EOF/parity/admission, body gate, WAF and RequestMirror regressions, real mock and
trait-object compilation, workspace all-target check/clippy, formatting and
applicable project guards. Full `cargo test --all` requires explicit user request.

Coordinator: `/root`. Technical analyst: `/root/body_contract_analysis`.
Independent value reviewer: `/root/body_contract_value_review`, configured as
gpt-6-astra with medium reasoning and independent original-source reading.
The analyst confirmed current debt; the independent reviewer conditionally
recommended presenting V1 for user confirmation. The review supports one
discoverable body contract/implementation owner, not reduced consumer fanout,
runtime improvements or complete plugin independence. Conditions: exactly four
unchanged definitions/defaults/documentation/error displays (all eight error
variants), migrate actual paths only, no compatibility exports or runtime changes,
preserve every listed lifecycle/admission contract, update relevant guidance, and
compile test/mock/all-target consumers with focused regressions after approval.
The initial value review identified no source-level feasibility blocker.
After explicit user confirmation, the separate implementer produced plan V1 and
the same independent reviewer approved its exact revision and frozen baseline.
Fresh compilation and focused regressions subsequently verified macro/object and
consumer wiring; these are executed results, not historical assumptions.

Fresh final reviewer `/root/body_contract_final_review` (gpt-6-astra, medium)
returned PASS with no blockers, independently verifying all 42 substantive files,
declaration identity, consumer/test body preservation, original check/Clippy and
eight test logs. Approval binds result manifest SHA256
`056eaf01062106f8d3f4206ed412060bdeb088391b1bf45363ae16fc5a744993`
and tracked diff SHA256
`7cd8c9c64f5768331a4f938292a126745819a69884aa56964ff9b672f57db935`.
Full tests/integration, commit and push were not run. Details and verification
limits are in `02-implementation.md` and `verification-results.json`.

This workflow did not edit source review artifacts. Existing untracked
`tasks/todo/waf-validation/` is unrelated and must remain untouched.
Another live workflow changed review-batch records and added new findings during
this read-only assessment. These are external changes, not this task's diff.
Implementation and verification used `/Volumes/ExtStore/ws6/Edgion-arch002`, an
isolated detached worktree. The user then explicitly authorized local integration
into `/Volumes/ExtStore/ws6/Edgion`, branch `feature-08-30`. Integration must
preflight the full frozen baseline, transfer the exact reviewed bytes, and preserve
unrelated changes. No commit, push, PR or external-change authority is granted.

Disposition: ARCH-002 implemented and independently accepted; exact reviewed
bytes integrated locally into feature-08-30. All 42 transferred file hashes match
the reviewed manifest, with diff hygiene and canonical ownership checks passing.
The user then explicitly authorized deleting the closed source finding. Its file
was removed after all 42 current result hashes were revalidated; the source batch
now marks ARCH-002 resolved and indexes its acceptance receipt. Original claim
hashes, independent verification history, unrelated candidates and primary
coverage remain intact. Closure receipt:
`tasks/review-architecture-20261009/TASK-ACCEPTANCE-ARCH-002-20261011.json`.
Searchable semantic ownership decision:
`skills/04-review/http-body/intentional-design/request-body-read-contract-ownership.md`.

## Evidence baseline

HEAD: `ef69a7804dc6764fed3eb5d725020cf27e88980c`.
SHA-256 below binds the input, original rules and decisive inspected scopes;
hashing a file does not claim every line was technically audited.

| File | SHA-256 | Inspected scope |
| --- | --- | --- |
| ARCH-002 finding | d358331fbd1262ba6c376773a3b09607dd7f16b976b7b2b04320c259ac387c09 | Full finding |
| VERIFICATION-ARCH-002.json | e47b66a41142107795b7496b4d20d8b0e6792f90ac40a15fa864647fc1ff50ec | Full historical receipt |
| AGENTS.md | cba9f532fa1c9f70e6a921e1a2331e7ca78c8210981a072a74cdebf4d85d3e42 | Applicable original rules |
| skills/00-design-rules.md | c78c86675ca6c72adb30d0f59727509486b582ef2c7f3318bf14e61745eff055 | Graded gates |
| src/plugins/runtime/traits/session.rs | 76b589ecae44bc709b1b1b5cec071d5b09f34a737fb76b8ab7dedf3844dfd71b | Imports and contract definitions |
| src/request_body/mod.rs | e861de4219c92a3bc994f48000e728d61d9f5eef1b1f2fc3901aed818bda219b | Full subsystem exports |
| src/request_body/storage.rs | e4b3f891f138f1bff617c95d5b3293151467c01707d71530db50eaeaaf8f0dac | Import, snapshot factory, resident/isolation implementation |
| src/request_body/state.rs | 47e036ef9a725668ee7b052ae2bb26c76c2baf98f49d830617fbb933cc8e91da | Imports and empty-reader selection |
| src/request_body/state/enabled.rs | 92e0ba87c8851d29fc01ee51def141662f16b0550db11c2c9fb875b41071850a | Imports and cached/empty/uncaptured selection |
| src/request_body/operation.rs | e7bd43f32b45613ee6bc425e696f8a8b2be531dda7cd91e867aadf0881c6b534 | Contract import and operation errors |
| src/request_body/body_scanners.rs | a54fba94ea1ee34d21f45e54c9d7687a3e6483e48ccbccaf5f87509fe5fcc95c | Contract import |
| src/plugins/runtime/session_adapter.rs | 54470cf128baa1a7510be8a49c1d35e70d5d680f540f43b39b09c0be8d0b7566 | Snapshot admission and EOF/parity consumption |
| src/plugins/http/request_mirror/plugin.rs | fd1f288cef9740573b8c73733a7fab83b22da97ef548c9ccff54d4f605b0e2ad | Detached reader isolation and lifetime |

Source paths abbreviated as `src/` belong to edgion-gateway. On resume revalidate
input, rules and affected source content before promoting the proposal.

## Approved continuation

On 2026-10-11 the user explicitly approved candidate V1. Implementation planning continues in isolated detached worktree `/Volumes/ExtStore/ws6/Edgion-arch002`, HEAD ef69a780, to avoid the original checkout's live review workflow. `01-design.md` and `01-baseline.json` freeze exact scope and source hashes for independent review. Source has eight BodyReadError variants; the earlier nine-variant count was a summary error and all eight are preserved. No production edits or builds have occurred during planning.
