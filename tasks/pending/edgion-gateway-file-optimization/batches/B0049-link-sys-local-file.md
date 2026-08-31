# B0049 — LinkSys Local-file Provider

## Scope
- Baseline: `a7b19937646d34bd383ba3cf5ddf57d2c8ec2978`
- Pipeline state: in-progress
- Active anchor crate: `edgion-gateway`
- Files: `edgion-gateway/src/link_sys/providers/local_file/{data_sender_impl.rs,mod.rs,rotation.rs}`
- Allowed support files: colocated unit tests in the three target files only; no cross-crate support is authorized before analysis proves necessity
- Forbidden files: every existing `edgion-tests/**` path, unrelated providers/runtime/template files, shared schemas without exact necessity evidence, and protected user paths
- Pre-existing protected paths: `tasks/pending/request-body-capability-next/`, `tasks/waf-todo/`, `waf-architecture-and-multi-pattern-matching.md`, and `tasks/handoff/`
- Exact commit allow-list: the three target files; `03-subtasks.md`; `07-changelog.md`; `edgion-gateway-file-optimization.md`; `deferred/INDEX.md`; any verified new deferred record; the B0048 and B0049 batch records

## Context and value gate
- Analysis subagent: `b0049_analysis`
- Coverage summary: reviewed all three files, logger construction, DataSender/access-log/SSL callers, rotation and backfill interaction, health/recovery, metrics, path resolution, cleanup, tests, and closed precedent.
- Concrete opportunity: size rotation makes an indexed archive the active file, so backfill can rename its inode while the writer keeps appending to a path future scans exclude. The size-index scanner can also panic on the active base name when its prefix and suffix overlap.
- Expected benefit: restore the base-path-only active-file invariant, remove the first-size-rotation panic, and make replacement and archive operations failure-safe without changing admission, accepted failed-flush size accounting, retention, or metrics.
- Regression surface: local writer rotation/recovery and deterministic filesystem tests only.
- Decision: change; a sender-wide graceful shutdown/drain contract is a deferred candidate.

## Implementation
- Implementation subagent: `b0049_implementation`
- Complexity: moderate
- Plan summary: archive the flushed base file and reopen the base path, require successful flush before writer replacement, preserve archive contents without overwrite, and add deterministic tempfile-based tests.
- Actual change: made every writer replacement flush-safe; implemented no-overwrite archive transactions, rollback, canonical-base attachment health, and base-path-only size/time rotation; fixed overlapping size-index parsing; and debounced repeated size-rotation failure warnings across batches.
- Behavior intentionally preserved: bounded non-blocking admission, first-healthy policy, pessimistic drop metrics, the accepted failed-flush `current_size` undercount, health debounce/recovery, retention matching, and external-rename handling.
- Tests added or updated: colocated tempfile tests only; no integration-test edit or gap is required.

## Independent review
- Reviewer: `b0049_independent_review`
- Verdict: LGTM after correction loops and a post-diagnostic re-review; all flush/replacement, no-overwrite archive, rollback, canonical-base health, recovery, parser-safety, and observability Must Fix items are cleared.
- Must Fix resolution: old buffers must flush before every replacement; archive creation cannot overwrite; dual failures and base reopen failures degrade health; recovery requires confirmed base attachment; repeated size-rotation failures log once per failure burst.
- Deferred observations: [`D0011`](../deferred/D0011-data-sender-graceful-shutdown-contract.md).
- Adjudication: `b0049_test_diagnostic` attributed the focused-test panic to the baseline size-index parser and identified a conflict with the binding failed-flush accounting precedent. Fresh read-only `b0049_precedent_adjudication` found no re-evaluation trigger, so the proposed exact accounting change and assertions were removed while the independent data-safety and rotation fixes were retained.

## Verification
| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Frozen integration paths before delegation | passed | 2,681 existing paths matched the manifest |
| Focused unit tests | passed | 18 local-file provider tests passed after diagnostic correction and final observability correction |
| Independent static review | passed | Final re-review cleared all file-operation, health, recovery, parser, precedent, and log-restraint Must Fix items |
| Format | passed | `cargo fmt --all`; final `cargo fmt --all -- --check` passed |
| Workspace compile | passed | `cargo check --workspace --all-targets`; a batch-created dead-code warning was corrected and the final run was clean |
| Workspace lint | passed | `cargo clippy --workspace --all-targets` exited zero; repository warnings remain pre-existing or belong to concurrent out-of-batch work |
| Repository guards | passed | Agent docs, SSA force, metrics inventory, Pingora source policy, and Gateway no-tracing checks passed |
| Frozen integration paths after verification | passed | 2,681 existing paths still matched the manifest; no integration-test edit or deferred integration gap was required |

## Handoff
- Ledger/changelog updated: yes
- Commit: `9693a166`
