# B0282 — HTTP ExtProc Attached

- Pipeline state: terminal; changed, verified and pending local commit.
- Active anchor: `edgion-gateway`; baseline `9b787978`, refreshed from `f8c265eb` with unchanged ExtProc code.
- Target: `edgion-gateway/src/plugins/http/ext_proc/attached.rs`.
- Analyst: `b0282_extproc_attached_analysis`; change. Prepare immutable normalized override modes once and share Arc<[ProcessingMode]> with each attached owner, removing enabled-override per-request Vec allocation/copy/normalization/free. No measured throughput claim.
- B0281 committed as `9b787978`; B0282 owns the serial lane. Implementer `b0282_implementation` authorized to execute its approved plan in the two frozen files only.
- Existing integration tests and user handoff/WAF paths are protected.

## Frozen scope and evidence

- Technical allow-list: `edgion-gateway/src/plugins/http/ext_proc/attached.rs` and `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`. The latter owns configuration preparation and owner construction; supporting edits do not establish full coverage of that file.
- Baseline blobs: attached `3670e9cce763446639762e9a5fdf136280df76bb`; plugin `b527b12354d17acc95d99424a1dfa94bc90c3eee`.
- Keep effective modes/phase/summary request-local, successive DEFAULT and exact normalized membership semantics, framing reservation, generation retention, cancellation/timeouts/failure policy, streaming and log ownership unchanged.
- Tests: attached unit module and exact nonnetwork framing/malformed-header tests; add shared-list/independent-owner and constructor-normalization regressions. Do not run the broad live-gRPC plugin test module.
- Independent incidental finding: request-body publication failures bypass owner terminalization in plugin.rs; defer separately after independent verification, never fold into ownership optimization.
- Record scope: this batch, B0281 SHA backfill, ledger/main/changelog, and independently verified standalone deferred tasks/backlinks/index. No integration files or protected user paths.

## Resume checkpoint

- Implementer returned a six-step plan: prepare the normalized Arc in the plugin constructor; retain it in attached owners; adapt direct fixtures; add sharing/independence and constructor-normalization tests; run exact nonnetwork tests; verify frozen invariants and scope.
- `b0282_plan_review` reported no blocking issue in its partial source check and confirmed the separate publication-failure finding, but failed with a provider usage-limit error before its final verdict. This is not plan approval. No implementation has started.
- Resume independent plan review when agent capacity is restored, obtain an explicit final verdict, then dispatch the original implementer. Do not bypass role separation or infer approval from the partial report.
- Next-candidate analyst `b0283_extproc_client_analysis` hit the same provider usage limit before a final report. Its partial TLS-policy candidate needs a completed, current-HEAD analysis; do not promote it.
- Capacity restored: `b0282_plan_review` completed Approved at unchanged 9b787978, no Must Fix. D0148 independently confirmed, not a blocker. Original implementer resumed; B0283 analysis also resumed without provider error.
- External d00e4be7 corrected the documented Pingora branch and resolved listener admission records; no source/dependency/integration files changed. Preserve B0147-B0150/D0100 updates and D0099/task removal. Plan reviewer reread original updated rules and reconfirmed Approved; no plan amendment required.
- Required doc guard then exposed an externally added link to an omitted task. Fresh `b0282_docs_diagnostic` confirmed pre-existing/external attribution and safe continuation via the exact missing standalone `tasks/todo/clarify-stream-plugin-udp-architecture-coverage.md`. Add that single pending investigation to bookkeeping scope; leave the listener page and runtime untouched. The new task does not claim UDP runtime conclusions.
- External 9b787978..b297af6e changed fifteen Gateway files: eleven changed and two no-change entries reopened, two already-pending entries retained. No new Gateway file. Preserve external D0078/D0086/D0108/D0116 resolutions and retire four removed task backlinks without recreation.
- Frozen M1/A1 verified against old manifest/Git fingerprints and new Git/worktree bytes plus mode; complete guard passes with 3,265 entries. An initial comparison rejected Ruby encoding tags; explicit byte comparison and identical SHA-256 established equality before any manifest edit. No integration writes.
- Implementation complete in the two allowed files, no plan deviation. At b297af6e the attached suite passed 9/9 and three exact nonnetwork plugin tests passed. Final reviewer `b0282_final_review` assigned; workspace verification active.
- Add the B0283 read-only candidate record and integration manifest provenance update to bookkeeping scope.
- Final review: `b0282_final_review` LGTM at b297af6e, no Must Fix or plan deviation. Original knowledge and relevant dependency chains independently verified. D0148 reconfirmed; D0149 separately records stale phase-exchange knowledge, also independently identified by B0283 analyst. Add its task/backlink to record scope.

## Approved plan

1. Prepare canonical normalized Arc slice in ExtProc::build_with_identity without changing source configuration or base mode.
2. Retain the shared slice directly in AttachedExtProc and clone its handle per request; effective mode and lifecycle remain request-local.
3. Adapt direct constructor fixtures within the frozen files only.
4. Add two-owner sharing/independence/lifetime and real-constructor normalization tests.
5. Run attached tests and the three named nonnetwork plugin tests; do not execute live-gRPC tests.
6. Obtain fresh final code review, run project gates and commit only allowed paths. Re-review any material plan deviation; recover only campaign-owned hunks.

## Verification and handoff

- Current b297af6e plus reviewed diff: attached tests 9/9 and three exact plugin tests passed; no live gRPC or integration tests executed.
- Workspace check passed (50.55s); Clippy passed (54.29s, existing warnings); formatting/format check, documentation, SSA, metrics, Pingora source, Gateway no-tracing, English, frozen paths and whitespace checks passed.
- Technical scope remained the two approved files; plugin.rs remains pending full coverage. Ledger and changelog updated. Commit pending; never push.
