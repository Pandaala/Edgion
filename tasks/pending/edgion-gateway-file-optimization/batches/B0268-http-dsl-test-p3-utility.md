# B0268 — HTTP DSL P3 Utility Tests

## Scope

- Pipeline state: terminal; changed, independently reviewed and verified.
- Active anchor: `edgion-gateway`.
- Baseline HEAD: `04511b57`.
- Target: `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs`.
- Target blob: `126308570133542686b798cdeb378903c008ef65`.
- Read scope includes the complete test file, every utility builtin/runtime path it exercises, prior B0129 coverage, adjacent tests, and current review/testing precedent.
- Existing `edgion-tests/**`, `tasks/handoff/`, `tasks/waf-todo/`, and `waf-architecture-and-multi-pattern-matching.md` remain protected.

## Analysis Outcome

- Decision: change; complex and plan-first.
- `join` currently allocates the full joined string before checking `max_string_len`, allowing delimiter amplification near one GiB. Preflight the exact UTF-8 byte length with checked add/multiply; return exact `StringTooLong`, or `len: usize::MAX` on arithmetic overflow, before allocation.
- `uuid()` returns a 36-byte runtime string without the canonical limit check. Route it through `checked_value_str` and cover 35-byte rejection plus 36-byte success.
- Fourteen target tests mutate process-global sensitive mode behind a module-local lock that does not coordinate other test modules. Replace that lock/guard with an exact subprocess helper; the parent must never mutate the global mode and every listed mode-specific assertion runs only in its uniquely marked child.
- Exact technical allow-list: `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` and `edgion-gateway/src/plugins/http/dsl/tests/p3_utility_tests.rs` only. D0094 and other builtin cleanup remain excluded.
- D0140 separately owns equivalent isolation for `runtime/log.rs`, `ctx.rs`, and `observe/access_log/log_ext.rs`. No knowledge or integration change is needed.

## Plan-first Gate

- Implementer returned a no-edit plan covering checked exact join preflight, UUID limit routing, four boundary regressions, and exact subprocess isolation for all 14 mode-mutating tests.
- Independent plan review initially found a Must Fix: exact subprocess exit success alone cannot detect a misspelled filter that runs zero tests.
- The revised plan adds a child-only unique completion marker checked alongside exit status, plus a private pure length helper with synthetic arithmetic-overflow tests. Independent re-review approved the revised plan with no remaining blocker.
- Implementation is now authorized within the exact two-file allow-list.

## Implementation and Final Review

- Added checked exact join-length preflight with overflow sentinel handling and post-check exact-capacity construction; routed UUID output through the canonical string limit.
- Added join exact/over-limit and synthetic overflow coverage plus UUID 35/36-byte boundaries.
- Migrated all 14 sensitive-mode tests to exact child processes with child-only global mutation, exit-status and unique completion-marker checks, recursion prevention, and captured diagnostics.
- Independent final code review: `/root/b0268_independent_final` PASS; no Must Fix. Reviewer confirmed exact Join arithmetic/allocation order, UUID limits, preservation of all 14 child-only assertion sets, and applicable red design rules.
- Earlier implementation stalled and was reassigned; its replacement had participated in plan review. This role-separation deviation is recorded explicitly; the fresh final reviewer participated in neither analysis, planning nor implementation. Future batches use separate fresh roles throughout.
- D0140 additionally requires child completion-marker verification to reject zero-test false positives.

## Verification

- Implementer: join helper 3/3, P3 utility 42/42, and full Gateway DSL 338/338 passed.
- Gateway all-target check, format, diff, agent-doc, SSA, metrics inventory, Pingora source, and Gateway no-tracing checks passed.
- Fresh full DSL filter `plugins::http::dsl::`: 373/373 passed, including lang/plugin/tests; the narrower `plugins::http::dsl::tests` filter above accounts for 338 tests.
- Coordinator locked workspace check/Clippy, format, agent-doc, SSA, metrics, Pingora source, no-tracing, English-only, diff and frozen integration checks passed. Clippy exited zero with 982 Gateway lib-test warnings.

## Handoff

- Ledger and changelog updated; one newly terminal file, builtins remains previously counted support coverage.
- Commit: `12cfc41d`; never pushed by the campaign.
