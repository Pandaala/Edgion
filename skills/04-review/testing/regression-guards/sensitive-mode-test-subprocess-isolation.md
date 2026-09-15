---
name: sensitive-mode-test-subprocess-isolation
description: Use when reviewing Gateway tests that mutate the process-global sensitive-output mode.
knowledge_type: regression-guards
---

# Sensitive-mode tests require exact subprocess isolation

## Conclusion and scope

Gateway unit tests that mutate the process-global sensitive-output mode must perform
the mutation and all dependent assertions in an isolated subprocess. A mutex local to
one test module does not protect the same mode from tests in other modules. The parent
test process must not mutate `SENSITIVE_MODE`.

The current shared helper is `edgion-gateway/src/test_subprocess.rs`. Sensitive-output
tests use a unique environment marker, a fully qualified `--exact` test name, and a
case identifier. The child prints a completion marker containing that exact test
identity only after its assertions finish. The parent requires a successful exit and
exactly one matching marker, and reports the exit status, stdout, and stderr on failure.

## Rationale and evidence

`edgion_resources::sensitive::SENSITIVE_MODE` is one `AtomicU8` shared by every test
module in the Gateway lib-test process. The former `ModeGuard` implementations in
`plugins/runtime/log.rs`, `ctx.rs`, and `observe/access_log/log_ext.rs` each held a
different module-local mutex. Those locks serialized only their own tests, so tests in
another module could change the mode between a store and the serialization assertion.

The correction routes the sensitive-mode tests in those modules, along with the
existing DSL, KeyAuth, RealIp, redirect, OTLP, and TLS subprocess tests, through the
same test-only helper. Production initialization and masking behavior are unchanged.

## Alternatives and limits

`--test-threads=1` is not a regression boundary because ordinary workspace tests run
in parallel. Restoring the previous global value is also insufficient because another
test can observe either the temporary value or the restoration. A process-wide test
mutex would require every future sensitive-mode mutator to find and use it; subprocess
isolation instead prevents the parent Gateway lib-test process from changing the mode.

This rule is established only for Gateway tests that intentionally mutate
`SENSITIVE_MODE`. A private policy seam that avoids the global entirely remains valid.
Tests of pure logic that neither mutate nor depend on the mode do not require a
subprocess. The `edgion-resources` sensitive tests run in a separate crate test
executable and therefore do not share the Gateway lib-test process.

## Verification and re-evaluation

Focused Gateway tests passed with eight test threads for:

- `plugins::runtime::log::tests` (25 tests)
- `ctx::sensitive_tests` (7 tests)
- `observe::access_log::log_ext::tests` (14 tests)
- DSL `p3_utility_tests::test_log*` (16 tests)
- the KeyAuth and RealIp isolated tests (1 test each)
- redirect tests (10 tests)
- OTLP log-sender tests (5 tests)
- TLS backend-address masking for termination and passthrough (2 tests)

Re-evaluate when another Gateway test writes `SENSITIVE_MODE`, the Rust test-harness
exact-name behavior changes, or the production masking policy becomes request-scoped
rather than process-global.

## Resolution references

- `isolate-remaining-sensitive-mode-gateway-tests` — fixed from the pending test task created 2026-09-10; the task carried no separate issue ID.
