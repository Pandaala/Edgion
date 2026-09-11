# B0053 — LinkSys Webhook Provider

## Scope
- Baseline: `9f2c442c5d2cc4f660426c2bb44bfeef5f0b1376`
- Pipeline state: reviewed; pending full verification and commit
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/providers/webhook/{client.rs,manager.rs,mod.rs,resolver.rs,runtime.rs}` plus necessary lifecycle support `edgion-gateway/src/link_sys/runtime/store.rs`
- Allowed support files: colocated unit tests in the six owned files only; no cross-crate support is authorized
- Forbidden files: every existing `edgion-tests/**` path, other providers, LinkSys runtime/template files, shared webhook schema/core files without exact necessity evidence, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the six owned files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `integration-freeze.tsv`; the B0052 and B0053 batch records; D0019–D0021 and legacy follow-up index

## Context and value gate
- Analysis subagent: `b0053_analysis`
- Coverage summary: reviewed all five Webhook provider files, LinkSys full/partial publication and lifecycle traits, direct manager consumers, managed outbound HTTP/TLS/egress/DNS/template paths, schemas/secrets, rate limits, tests, and binding precedents.
- Concrete opportunity: authoritative full-set replacement skips shutdown for Webhook old clients, so omitted keys leave callable entries in the global manager; an unconditional delayed shutdown can also remove a newer same-key registration.
- Expected benefit: make omission a real removal for every provider while preserving same-key Webhook replacement and preventing old generations from erasing new registrations.
- Regression surface: LinkSys full-set old-client shutdown ordering and Webhook manager registration identity; partial removal and atomic publication must remain unchanged.
- Decision: change `runtime/store.rs`, `webhook/client.rs`, and `webhook/manager.rs`; examine the other three Webhook files with no expected change. Defer the shared sliding-window issue.

## Implementation
- Implementation subagents: `b0053_implementation`; `b0053_implementation_recovery` resumed the partial work after two model-capacity failures
- Complexity: complex; lifecycle concurrency and generation identity span three behavioral files.
- Plan summary: synchronously reserve a bounded authoritative source-event sequence before resource publication and task spawning; reject stale or duplicate dispatches under one Webhook manager mutation guard held across the complete provider transition; prepare Webhook clients before atomic one-key or full-map manager commits and infallible runtime publication; specialize Webhook store paths and remove its generic lifecycle implementation; cover reversed task-polling order, full/partial interleavings, failure preservation, atomic publication, and sequence overflow.
- Actual change: four authorized implementation files now reserve bounded source-event sequences before publication/spawn, serialize accepted LinkSys transitions through the Webhook manager, reject stale dispatches before side effects, publish prepared Webhook replacements atomically, retire omissions/deletions, keep Webhook outside the generic spawned lifecycle path, and remove its obsolete adapter exports.
- Behavior intentionally preserved: outbound client/TLS/egress/DNS/template construction, request-time manager lookup, other providers' lifecycle implementations, and the existing integration-test tree were not modified by B0053.
- Tests added or updated: colocated unit tests cover sequence bounds, stale/equal dispatch rejection, reversed polling order, full/partial interleavings, atomic manager publication, identity preservation, failure carry-over, omission/deletion, and replacement without a missing window.

## Independent review
- Plan reviewer: `b0048_independent_review`
- Plan verdict: Approved after six revisions
- Plan Must Fix resolution: the final plan assigns sequence synchronously before spawn, rejects older dispatches before preparation or any provider mutation, holds one accepted guard across the full transition, removes the silent Webhook lifecycle implementation, and defines equality, multi-operation, overflow, and publication semantics with deterministic tests.
- Final code reviewer: `b0053_final_review`
- Final verdict: Approved after correction re-review
- Must Fix resolution: production and tests now share the synchronous source-event boundary, with deterministic real-entry interleavings and overflow non-publication coverage; the unused manager lifecycle facade and re-exports were removed.
- Deferred observations: D0019 records Webhook window/time coherence, D0020 records the cross-crate stale lifecycle documentation, and D0021 records same-key partial replacement across provider types.
- Adjudication: all three are real, non-duplicate, and wider than B0053; none requires modifying an existing integration test in this batch

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Gateway library compile | passed | `cargo check -p edgion-gateway --lib` |
| LinkSys unit tests | passed | 184 tests after final formatting |
| Webhook manager tests | passed | 21 tests after correction |
| LinkSys store tests | passed | 17 tests after correction |
| Webhook client tests | passed | 1 test |
| Gateway no-tracing guard | passed | no violation |
| Diff whitespace check | passed | `git diff --check` |
| Frozen integration paths after implementation | passed after authorized rebaseline | External HEAD `9830fb2a` changed seven committed `edgion-tests/**` files; the user identified them as concurrent-agent work and explicitly instructed the campaign to ignore that external drift, so only those seven committed fingerprints were advanced and are frozen again |
| Full formatting | passed | `cargo fmt --all` |
| Workspace compile | passed | `cargo check --workspace --all-targets` |
| Workspace Clippy | passed | exit 0 with existing repository warnings |
| Agent documentation | passed | 1,114 files, 36 skill entries, and 711 source-path scopes checked |
| SSA force guard | passed | 1,432 files checked; one existing allowlisted site |
| Metrics inventory | passed | 103 metrics in sync |
| Pingora source policy | passed | 16 fork packages on `edgion_v3` at one locked revision |
| Final Gateway no-tracing guard | passed | data-plane boundary and cold-function registry clean |
| Final frozen integration paths | passed | the user-authorized concurrent baseline remained unchanged during B0053 verification |

## Handoff
- Ledger/changelog updated: yes; B0053 closes four files as changed, one as examined-no-change, and one as deferred
- Commit: `9e34ab8f`
