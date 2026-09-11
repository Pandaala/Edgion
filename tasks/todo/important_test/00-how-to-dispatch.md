# How to dispatch an agent on one of these tasks

| Key | Value |
|-----|-------|
| Created | 2026-08-04 |
| Applies to | every task file in this directory |
| Why it exists | the work that produced these tasks shipped, and then caught, four separate classes of bad test. Each rule below is one of them. |

Dispatch message can be short — point at the task file and at this one:

> Implement `tasks/todo/important_test/0N-<name>.md`. Follow
> `tasks/todo/important_test/00-how-to-dispatch.md` — its rules are not
> optional and its report contract is what I read.

Everything below is what that reference buys you.

---

## 1. The bar for a test to count as coverage

**A negative claim needs a real assertion.** "No second connection was opened",
"no body byte reached the upstream", "no retry happened" — each needs a bounded
`expect_none` **plus** an exact whole-log count. A bound like `count <= 1` is
satisfied by zero and proves nothing. This defect was found twice in this suite.

**Prove the thing happened before asserting how it ended.** Every scenario must
first establish that the upstream received the request (or the client got the
response, as applicable). A test that never reached the code under test passes
silently otherwise. The fork harness has an `ExercisedUpstream` drop guard for
exactly this — do not disarm it except through the announced skip path.

**A promptness or ordering claim needs both bounds.** An upper bound alone
cannot fail if the lower bound never occurred.

**A skipped combination announces itself with its reason** and does not count as
coverage.

**Verify the wire, not the intent.** An assertion recorded *before* the action
it claims to observe cannot fail. One backend in this suite recorded "reset
sent" before calling `send_reset`, and passed in every run where the library
silently sent no reset at all.

**A test whose backend cannot emit the shape it names will alternate.** If a
case passes and fails on alternating runs, suspect that first — measure what the
wire actually carries before touching the assertion. One case here was measured
over 960 runs: the shape it was named for occurred **zero** times.

## 2. Prove the test discriminates

For every test added or materially changed: **revert the production behavior it
targets, observe the failure, restore.** Report the observed failure, not the
intention. Do not claim a mutation-tested fix without running the mutation — an
agent on this work once reported a repro that passed against the unfixed code,
i.e. was worthless, and said so; that is the standard.

If the test is meant to pin a *conditional* (a guard, a branch), mutate the
condition, not just delete the feature. Deleting a whole code path often leaves
tests green for the wrong reason.

## 3. Prove the test is stable

Run every new or changed test **in isolation at least 10 times**
(`--exact --test-threads=1` for Rust unit/integration tests) and report the
actual pass count. Whole-binary runs are not evidence for an individual test:
this suite once shipped a case that passed in-suite and failed **5 of 6**
isolated runs, and whose assertion was vacuous besides.

For the Edgion integration suite, run the selected scenario directly and retain
its canonical report:

```bash
./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r <R> -i <I>
```

A run that reports no summary is a **collapse**, not a pass. Say so; do not
count it. The former claim that a cleanup SIGKILL retained the Gateway's listen
socket was disproved; see the evidence boundary in
`skills/04-review/testing/regression-guards/integration-suite-lifecycle-errors-must-fail.md`.
If a collapse returns, preserve both adjacent runs' teardown and startup logs
instead of hiding the failure with an extra cleanup command.

## 4. When a test fails

**Diagnose before adjusting.** A failing assertion is more likely to be right
than convenient. In this work, every time an assertion was interrogated instead
of adjusted, it was pointing at something real — including one Critical that a
weakened assertion would have buried.

**If the failure is a product defect, say so and leave it failing.** Do not
weaken the assertion to get green. Record it as a finding with the evidence.

**If a behavior cannot be observed without changing production code, that is a
finding to report, not a licence to change it.** Say what you would need and
why.

## 5. Scope discipline

Touch only what the task file names. In particular:
- test-harness tasks must not modify production crates;
- (resolved 2026-08-04) the former `04-fix-the-product-before-writing-the-test.md`
  held two gaps that must not get tests until a decision landed; both decisions
  have landed — gap 7 fixed in the fork and tested there, gap 8 accepted with
  the terminate-shape test ban recorded in
  `skills/04-review/h2-grpc/intentional-design/terminate-upstream-rst-not-rate-limited.md`. The
  gap-8 ban still holds: do not add a test pinning "N terminates → N resets";
- the fork's `test_basic.rs` / `test_upstream.rs` need a local openresty and are
  inert here. Do not try to run them, and do not re-add them as a task.
- the fork's `test_basic.rs` / `test_upstream.rs` need a local openresty and are
  inert here. Do not try to run them, and do not re-add them as a task.

## 6. Correct the record when a claim turns out wrong

These task files and `tasks/todo/transport-coverage-observability-gaps.md`
contain claims written before the behavior was observable. Several have already
been disproved by observation — the proxy sending FIN rather than RST, a
proposed fix that the dependency's API could not express, a defect that was a
test bug all along.

When you disprove one: **correct it in place, keep the original claim visible,
and say what disproved it.** Do not silently rewrite — the correction is the
valuable part, and the next person needs to know the file was once wrong about
this.

## 7. Report contract

Write the full report to a scratch file; reply with under 15 lines:

- **Status** and what changed, per item in the task file
- **Discrimination evidence** per test (what you mutated, what failed, restored)
- **Isolation counts** (test × runs × passes)
- **Suite results** verbatim, including any collapse
- **What you could NOT do**, with the reason — this is not optional, and a
  report claiming everything worked is treated with more suspicion than one
  that names its gaps
