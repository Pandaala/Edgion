# Isolate Remaining Sensitive-mode Gateway Tests

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-10 |
| Status | pending |
| Type | test |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Eliminate cross-module races in the remaining Gateway tests that mutate the process-global sensitive-output mode behind unrelated local locks.

## Requirements

- Inventory every mode-mutating test in `edgion-gateway/src/plugins/runtime/log.rs`, `edgion-gateway/src/ctx.rs`, and `edgion-gateway/src/observe/access_log/log_ext.rs` plus any newly discovered Gateway caller.
- Replace module-local mutex serialization with the established exact-test subprocess pattern, or with an existing private policy seam that avoids reading/writing the global atomic entirely.
- Parent tests must never mutate the process-global sensitive mode.
- Exact child selection must use a unique environment marker and the fully qualified test name; child execution must not recursively spawn.
- Require both successful child exit and a unique completion marker emitted after the child assertions, so an incorrect exact filter cannot pass by running zero tests.
- Preserve Off/Replace/Wrap behavior, default-mode behavior, plaintext-absence assertions, bounded PluginLog behavior, and all access-log redaction/blocklist invariants.
- On child failure, surface exit status, stdout, and stderr for diagnosis.
- Do not weaken the production global mode, add test-only production state, or rely on `--test-threads=1`.
- Do not modify existing `edgion-tests/**` files.

## Scope

### In scope

- The three Gateway test modules listed above and directly shared test-only subprocess helpers when reuse is justified.

### Out of scope

- Production sensitive-mode semantics, public APIs, DSL utility tests, logging formats, and integration tests.

## Acceptance criteria

- No in-scope test mutates the process-global sensitive mode in the parent Gateway test process.
- Mode-specific assertions remain exact for Off, Replace, and Wrap.
- The tests pass under ordinary parallel workspace test execution without local-lock races.
- Focused tests and all repository guards pass.
