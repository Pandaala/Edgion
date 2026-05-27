# Snapshot Testing Skill + B1 Pilot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a new `skills/05-testing/05-snapshot-testing.md` skill plus a quantified B1 pilot converting `decision_log_tests.rs` to use `expect-test` snapshots, so the org has both the rules and a concrete reference for future snapshot-testing conversions.

**Architecture:** Two intertwined deliverables. (1) A `PluginLog::render_for_test()` helper makes the log usable as a snapshot input. (2) Five of six tests in `decision_log_tests.rs` collapse their `log.contains(...)` probes into single `expect![[...]]` blocks; the sixth retains its negative probes alongside the snapshot to preserve invariant intent. (3) A new authoritative skill file documents the decision rubric (green/yellow/red light) and operations manual (refill workflow, volatile-field escape hatches).

**Tech Stack:** Rust + `expect-test = "1.5"` (dev-dependency only); existing in-tree `PluginLog` / `LogBuffer` / `ULogBuffer` types from `src/core/gateway/plugins/runtime/log.rs` and `src/core/gateway/observe/logs/buffer.rs`; `cargo test` module-path filter form (this is a single-crate `[package]` project, NOT a workspace — see Cargo.toml line 1).

**Source spec:** `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md` (committed at `b68cfaa`).

**Commit discipline:** Per `feedback_test_cover_execution` + spec § 4 Step 4, **no per-task commits**. Each task `git add`s its files; the final task (Task 9) runs main-session verification then asks the user to approve a single end-of-pipeline commit. Subagents NEVER commit.

**Subagent verify scope (per `feedback_no_clippy_no_build`):** `cargo fmt --all` + `cargo check --lib --tests` + targeted `cargo test`. NO clippy, NO `cargo build`, NO `cargo test --all`. Main session adds clippy + `make check-agent-docs` in Task 9.

---

## File Structure

Files this plan creates or modifies:

| Path | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify (line 202, `[dev-dependencies]` block) | Add `expect-test = "1.5"` |
| `src/core/gateway/plugins/runtime/log.rs` | Modify (extend `impl PluginLog`) | Add `#[cfg(test)] pub fn render_for_test(&self) -> String` |
| `src/core/gateway/plugins/runtime/tests/log_test.rs` *or* inline `mod tests` in `log.rs` | Create/modify | Unit test for `render_for_test` (empty / log-only / ulog-only / both buffers) |
| `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs` | Rewrite 6 test bodies | 5 full snapshot conversions + 1 partial (test #6) |
| `skills/05-testing/05-snapshot-testing.md` | Create | New skill: decision rubric + operations manual |
| `skills/05-testing/SKILL.md` | Modify (Part 1 table) | Register the new skill file |
| `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md` | Append to § 7 Outcomes | Final measurements + notes (after pilot lands) |
| `docs/superpowers/plans/2026-05-27-snapshot-testing-skill-and-pilot.md` | This file | The plan itself |

The `render_for_test` helper lives next to `PluginLog` (in `log.rs`, gated with `#[cfg(test)]`) — it is owned by the log module, used by tests in many modules. Do NOT scatter equivalent helpers across each test file.

---

## Task 1: Add expect-test dependency

**Files:**
- Modify: `Cargo.toml` (the `[dev-dependencies]` block — confirm at line 202 in the current revision)

- [ ] **Step 1: Read the current `[dev-dependencies]` block to identify the right insertion point**

Run: `grep -n "^\[dev-dependencies\]" Cargo.toml && sed -n '202,225p' Cargo.toml`
Expected: a line `[dev-dependencies]` followed by entries like `tempfile = "3"`, `mockall = "0.14.0"`, `proptest = "1"`, etc. Note the alphabetical-by-name ordering used in that block.

- [ ] **Step 2: Insert `expect-test = "1.5"` into the block in alphabetical order**

Edit `Cargo.toml`. Add this line under `[dev-dependencies]` in the right alphabetical position (between `dotenvy` and `h2`, or wherever fits the existing ordering):

```toml
expect-test = "1.5"
```

Match the style of neighbors (no `version =` table form, just bare `name = "minor"`).

- [ ] **Step 3: Verify Cargo resolves the dep without breaking the workspace**

Run: `cargo check --lib --tests 2>&1 | tail -20`
Expected: compile succeeds (any warnings are pre-existing, not introduced by this dep). If the dep itself fails to resolve (offline, version yanked), stop and report.

- [ ] **Step 4: Record the resolved version**

Run: `grep -A1 '^name = "expect-test"$' Cargo.lock | head -2`
Expected: a line like `version = "1.5.x"`. Note this number — it goes into the PR description and into the spec's § 7 Outcomes in Task 8.

- [ ] **Step 5: Stage the Cargo files**

```bash
git add Cargo.toml Cargo.lock
```

Do NOT commit. Subagents never commit (spec § 4 Step 4).

---

## Task 2: Add `PluginLog::render_for_test` with TDD

**Files:**
- Modify: `src/core/gateway/plugins/runtime/log.rs` (extend `impl PluginLog` near line 187, add `#[cfg(test)] mod render_for_test_tests` at the bottom)

The `render_for_test` helper is the prerequisite for every snapshot conversion that follows. Build it test-first.

- [ ] **Step 1: Write the failing unit tests**

Append to `src/core/gateway/plugins/runtime/log.rs` (after the last existing `impl` block, before any existing `#[cfg(test)] mod tests`). If a `#[cfg(test)] mod tests` already exists at the bottom of the file, ADD the new tests inside it; do NOT create a sibling module.

Check first:
```bash
grep -n '#\[cfg(test)\] *$\|^#\[cfg(test)\]$\|^mod tests\|^#\[cfg(test)\]\s*\nmod tests' src/core/gateway/plugins/runtime/log.rs | head -5
```

Whether you find an existing `#[cfg(test)] mod tests` or not, place these four tests inside the test module (creating it if absent):

```rust
#[cfg(test)]
mod render_for_test_tests {
    use super::*;
    use crate::core::gateway::observe::logs::buffer::{LogBuffer, ULogBuffer};

    #[test]
    fn render_for_test_empty_buffers_returns_empty_string() {
        let log = PluginLog::new("test");
        assert_eq!(log.render_for_test(), "");
    }

    #[test]
    fn render_for_test_log_only_joins_entries_with_newline() {
        let mut log = PluginLog::new("test");
        log.push("first");
        log.push("second");
        assert_eq!(log.render_for_test(), "first\nsecond");
    }

    #[test]
    fn render_for_test_ulog_only_when_log_is_none() {
        // Force the ulog-only branch by direct field assignment.
        let mut log = PluginLog::new("test");
        let mut u = ULogBuffer::new();
        u.push("only-ulog");
        log.ulog = Some(u);
        assert_eq!(log.render_for_test(), "only-ulog");
    }

    #[test]
    fn render_for_test_both_buffers_separated_by_marker() {
        // Construct both buffers directly to make the separator behavior
        // testable independent of the tier-1 overflow threshold.
        let mut main = LogBuffer::new();
        main.push("main-a");
        main.push("main-b");
        let mut user = ULogBuffer::new();
        user.push("user-x");
        let log = PluginLog {
            log: Some(main),
            ulog: Some(user),
            ..PluginLog::new("test")
        };
        assert_eq!(log.render_for_test(), "main-a\nmain-b\n--- \nuser-x");
    }
}
```

Note: these tests use `PluginLog::new`, `push`, and direct field assignment. All three are pub today (`log`, `ulog` fields are `pub Option<...>` at lines 105 and 110). If a future refactor makes those fields private, the test that uses direct assignment will need to call `push` enough to overflow tier-1; flag it but don't block the plan.

- [ ] **Step 2: Run the new tests to verify they fail (compilation error — method does not exist yet)**

Run: `cargo test -p edgion core::gateway::plugins::runtime::log::render_for_test_tests 2>&1 | tail -20`
Expected: compilation error `no method named 'render_for_test' found for struct 'PluginLog'`. This proves the tests are reaching the right type and only the implementation is missing.

- [ ] **Step 3: Implement `render_for_test` on `PluginLog`**

Append inside the existing `impl PluginLog { ... }` block in `src/core/gateway/plugins/runtime/log.rs`, just before the closing `}` of that impl. Find the impl by `grep -n '^impl PluginLog {' src/core/gateway/plugins/runtime/log.rs` (should be line 187). Add this method as the LAST method inside that block:

```rust
    /// Render this log as a single `String` suitable for `expect-test`
    /// snapshot assertions.
    ///
    /// Entries inside one tier are joined by `\n`. When both tier-1 (`log`)
    /// and tier-2 (`ulog`) carry entries, they are separated by the literal
    /// marker `\n--- \n` — a sequence reviewers can grep for and visually
    /// distinguish in `git diff`. Empty `PluginLog` yields `""`.
    ///
    /// `#[cfg(test)]` is deliberate: this format is for tests only. A
    /// production `Display` impl is intentionally NOT provided to prevent
    /// accidental `println!("{}", plugin_log)` from leaking into hot paths.
    ///
    /// See `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md`
    /// § 2.2 for the separator-format decision rationale.
    #[cfg(test)]
    pub fn render_for_test(&self) -> String {
        let main: Vec<&str> = self
            .log
            .iter()
            .flat_map(|b| b.entries())
            .collect();
        let user: Vec<&str> = self
            .ulog
            .iter()
            .flat_map(|b| b.entries())
            .collect();
        match (main.is_empty(), user.is_empty()) {
            (true, true) => String::new(),
            (false, true) => main.join("\n"),
            (true, false) => user.join("\n"),
            (false, false) => {
                format!("{}\n--- \n{}", main.join("\n"), user.join("\n"))
            }
        }
    }
```

- [ ] **Step 4: Run the unit tests — expect all four to pass**

Run: `cargo test -p edgion core::gateway::plugins::runtime::log::render_for_test_tests 2>&1 | tail -20`
Expected:
```
running 4 tests
test ... ok
test ... ok
test ... ok
test ... ok

test result: ok. 4 passed; 0 failed
```

If `render_for_test_both_buffers_separated_by_marker` fails, inspect the actual output — most likely the separator constant in the impl drifted from the test expectation. Fix one or the other (both must use `\n--- \n` exactly).

- [ ] **Step 5: Format + targeted check (subagent verify scope)**

Run: `cargo fmt --all && cargo check --lib --tests 2>&1 | tail -10`
Expected: no errors. Warnings on unrelated files are fine; warnings introduced in `log.rs` must be fixed.

- [ ] **Step 6: Stage the file**

```bash
git add src/core/gateway/plugins/runtime/log.rs
```

Do NOT commit.

---

## Task 3: Baseline measurement of `decision_log_tests.rs`

This task captures **Before** numbers for the quantification table. It must run BEFORE any test bodies are rewritten.

**Files:**
- Read only (no edits): `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`

- [ ] **Step 1: Confirm the test file is currently green**

Run: `cargo test -p edgion dsl::tests::decision_log_tests 2>&1 | tail -15`
Expected:
```
test result: ok. 6 passed; 0 failed
```
If any test fails NOW, stop — the baseline must be from a clean tree.

- [ ] **Step 2: Measure body lines (excluding `#[test]` attribute and `fn ... {` and `}` closing lines)**

Run: `awk '/^#\[test\]$/,/^}$/' src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs | wc -l`
Expected output: a number around 70–78 (the spec estimated ~70, the reviewer measured lines 10–81 = 72). Record the exact number; it goes in the spec § 7 Outcomes.

- [ ] **Step 3: Count assertion calls**

Run: `grep -cE 'assert_eq!|assert!' src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`
Expected: a number around 20 (the file's contents show 6 × `assert_eq!(result, ...)` + ~14 × `assert!(...contains...)` ≈ 20; the spec § 4 Step 3 estimate of "~28" included expanded positive+negative probes, but raw assertion-call count is fewer because `assert!(log.contains(X))` counts as one).

Record:
- `BASELINE_BODY_LINES = <number>`
- `BASELINE_ASSERTION_CALLS = <number>`
- `BASELINE_CONTAINS_PROBES = <number from: grep -c '\.contains(' on the file>`

- [ ] **Step 4: Save the numbers to a scratch note**

These numbers go into the spec § 7 Outcomes in Task 8 and into the final commit message in Task 9. Keep them in your scratchpad / TodoWrite item — do NOT write them to a file in the repo (the spec is the canonical home).

- [ ] **Step 5: No code change in this task; nothing to stage.**

---

## Task 4: Convert tests #1–#5 to expect-test (full conversion)

The five tests to fully convert: `test_decision_log_next_emits_ok`, `test_decision_log_implicit_return_emits_ok`, `test_decision_log_deny_emits_status`, `test_decision_log_terminate_emits_deny_once`, `test_decision_log_grpc_error_emits_deny`.

**Files:**
- Modify: `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`

For each of the five tests apply the SAME transformation rule:

- Keep `assert_eq!(result, <enum>)` verbatim — it asserts a semantic enum.
- Replace every `assert!(log.contains(...))` + `assert!(!log.contains(...))` line in the body with ONE `expect![[r#"<placeholder>"#]].assert_eq(&log.render_for_test());` line.
- The placeholder is filled by `UPDATE_EXPECT=1` in Step 2 below.

- [ ] **Step 1: Add the expect-test import and rewrite the five tests**

Edit `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`. Make these changes:

**1a. Add the use statement** under the existing `use super::*;` at line 8:

```rust
use super::*;
use expect_test::expect;
```

**1b. Replace test #1 body** (lines 11–17 area):

```rust
#[test]
fn test_decision_log_next_emits_ok() {
    let vm = make_vm("return next()");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    expect![[r#"<FILL VIA UPDATE_EXPECT>"#]].assert_eq(&log.render_for_test());
}
```

**1c. Replace test #2 body** (`test_decision_log_implicit_return_emits_ok`, lines 19–25 area):

```rust
#[test]
fn test_decision_log_implicit_return_emits_ok() {
    let vm = make_vm("let x = 1");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, good_next());
    expect![[r#"<FILL VIA UPDATE_EXPECT>"#]].assert_eq(&log.render_for_test());
}
```

**1d. Replace test #3 body** (`test_decision_log_deny_emits_status`, lines 27–34 area):

```rust
#[test]
fn test_decision_log_deny_emits_status() {
    let vm = make_vm(r#"return deny(403, "blocked")"#);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(403, "blocked"));
    expect![[r#"<FILL VIA UPDATE_EXPECT>"#]].assert_eq(&log.render_for_test());
}
```

**1e. Replace test #4 body** (`test_decision_log_terminate_emits_deny_once`, lines 36–44 area):

```rust
#[test]
fn test_decision_log_terminate_emits_deny_once() {
    let vm = make_vm("return terminate()");
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, PluginRunningResult::ErrTerminateRequest);
    expect![[r#"<FILL VIA UPDATE_EXPECT>"#]].assert_eq(&log.render_for_test());
}
```

**1f. Replace test #5 body** (`test_decision_log_grpc_error_emits_deny`, lines 46–59 area):

```rust
#[test]
fn test_decision_log_grpc_error_emits_deny() {
    let vm = make_vm(r#"return grpc_error(7, "permission denied")"#);
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(
        result,
        PluginRunningResult::ErrGrpcError {
            grpc_status: 7,
            message: Some("permission denied".to_string()),
        }
    );
    expect![[r#"<FILL VIA UPDATE_EXPECT>"#]].assert_eq(&log.render_for_test());
}
```

Leave test #6 untouched in this task — it's the partial conversion handled in Task 5.

- [ ] **Step 2: Run with `UPDATE_EXPECT=1` to refill the placeholders**

Run: `UPDATE_EXPECT=1 cargo test -p edgion dsl::tests::decision_log_tests 2>&1 | tail -25`
Expected: all 6 tests pass (test #6 still uses its old form; the five rewritten tests had `<FILL VIA UPDATE_EXPECT>` placeholders that expect-test now replaces in source with the actual `render_for_test` output).

If the run fails to compile, common culprits:
- `expect_test::expect` not imported → check Step 1a
- `run_vm_policy_log` returns `(_, PluginLog)` not `(_, String)` → confirm `.render_for_test()` is being called on it
- `render_for_test` is `#[cfg(test)]` and the import path is wrong → these tests run under `#[cfg(test)]` so should be visible

- [ ] **Step 3: Read every refilled snapshot — MANDATORY**

Run: `git diff src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`
For each `expect![[r#"..."#]]` block now containing real content, verify:

- The content starts with the expected decision token (`OK; `, `Deny 403; `, `Deny terminate; `, `Deny grpc 7; `).
- For tests where the previous `assert!(!log.contains("dsl match"))` existed, confirm the snapshot does NOT contain the substring `dsl match`. If it does, this is a real regression — STOP, do not proceed.
- Confirm no surprises like stack traces, timestamps, addresses, or other non-deterministic substrings. Per spec § 4 Step 1, this file's inputs are deterministic; if the snapshot is volatile, fall back to the conditional probe described in skill § 3 (Task 7 covers the skill content).

- [ ] **Step 4: Run the tests again without `UPDATE_EXPECT` to confirm they pass on read-back**

Run: `cargo test -p edgion dsl::tests::decision_log_tests 2>&1 | tail -15`
Expected:
```
test result: ok. 6 passed; 0 failed
```

If any of the five refilled tests fails now (despite passing under `UPDATE_EXPECT=1`), the run is non-deterministic between invocations — escalate per spec § 4 Step 1's fallback (skill § 3 escape hatches; do not jump to `insta` for the pilot).

- [ ] **Step 5: Format + targeted check**

Run: `cargo fmt --all && cargo check --lib --tests 2>&1 | tail -10`
Expected: no errors introduced.

- [ ] **Step 6: Stage**

```bash
git add src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs
```

Do NOT commit.

---

## Task 5: Convert test #6 (partial conversion — preserve negative invariants)

Test #6 (`test_decision_log_runtime_error_emits_only_err_entry`) carries four distinct invariants: one positive (`err: ` IS present) and three negative (`OK`, `Deny`, `dsl match` are all absent). The full-conversion approach would lose the negative-invariant explanations encoded in `assert!`'s panic messages. Per spec § 4 Step 2 / review finding I1: keep the negatives, ADD the snapshot.

**Files:**
- Modify: `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs` (test #6 only)

- [ ] **Step 1: Rewrite test #6 with both negative probes AND snapshot**

Replace the existing test #6 body (current lines ~61–81) with:

```rust
#[test]
fn test_decision_log_runtime_error_emits_only_err_entry() {
    // max_steps: 3 forces StepLimitExceeded before any explicit return.
    let vm = make_vm_with_limits(
        "let a = 1\nlet b = 2\nlet c = 3\nlet d = 4",
        VmLimits {
            max_steps: 3,
            ..Default::default()
        },
    );
    let (result, log) = run_vm_policy_log(&vm, MockPluginSession::new(), &DslErrorPolicy::Deny);
    assert_eq!(result, err_response_body(500, "DSL execution error"));

    // PARTIAL CONVERSION (spec § 4 Step 2 / review finding I1):
    // The exact-string snapshot below guards the precise log format.
    // The three negative `!contains` probes below the snapshot carry
    // invariant-explanation messages a reviewer reads in a failure
    // output — these protect against a regression that re-introduces
    // a decision token on the error path, where the snapshot alone
    // would only show "string differs" without the "why".
    expect![[r#"<FILL VIA UPDATE_EXPECT>"#]].assert_eq(&log.render_for_test());

    // Invariants the snapshot alone cannot articulate to a reviewer:
    assert!(
        !log.contains("OK"),
        "error path must not emit 'OK' decision token"
    );
    assert!(
        !log.contains("Deny"),
        "error path must not emit a 'Deny' decision token"
    );
    assert!(
        !log.contains("dsl match"),
        "stale 'dsl match' entry must not appear"
    );
}
```

Note: `log.contains(...)` here uses the existing `#[cfg(test)] fn contains(&self, substr: &str) -> bool` on `PluginLog` (defined at `src/core/gateway/plugins/runtime/log.rs:345` per the reviewer's findings). DO NOT change those `assert!(!log.contains(...))` lines to `assert!(!log.render_for_test().contains(...))` — the existing form is already test-gated and avoids a redundant String allocation per assertion.

The original positive `assert!(log.contains("err: "))` is DROPPED, because the snapshot will exactly match (and therefore include) the `err: ` substring; keeping the positive `contains` after the snapshot would be redundant. The spec § 4 Step 2 / I1 finding specifically named the positive `err: ` probe + three negative probes as the four invariants; only the negative ones survive past the snapshot.

- [ ] **Step 2: Refill the placeholder**

Run: `UPDATE_EXPECT=1 cargo test -p edgion dsl::tests::decision_log_tests::test_decision_log_runtime_error_emits_only_err_entry 2>&1 | tail -10`
Expected: test passes; the placeholder is replaced with the real `err: ...` log content.

- [ ] **Step 3: Read the refilled snapshot — MANDATORY**

Run: `git diff src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`
Verify:
- The snapshot contains a `err: ` prefix entry.
- The snapshot does NOT contain `OK`, `Deny`, or `dsl match` — if any appears, the three negative `assert!`s below the snapshot will catch it, but you should still notice during diff review.
- The snapshot is a finite, deterministic string (no addresses, no timestamps).

- [ ] **Step 4: Run the full file to confirm read-back passes**

Run: `cargo test -p edgion dsl::tests::decision_log_tests 2>&1 | tail -15`
Expected: 6 passed, 0 failed.

- [ ] **Step 5: Format + targeted check**

Run: `cargo fmt --all && cargo check --lib --tests 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 6: Stage**

```bash
git add src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs
```

Do NOT commit.

---

## Task 6: Measure After numbers and prepare Outcomes

This task captures **After** numbers and prepares the content that will be appended to the spec's § 7 Outcomes in Task 8.

**Files:**
- Read only on `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs` for measurement.

- [ ] **Step 1: Measure body lines**

Run: `awk '/^#\[test\]$/,/^}$/' src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs | wc -l`
Record as `AFTER_BODY_LINES`.

- [ ] **Step 2: Count assertion calls**

Run:
```bash
grep -cE 'assert_eq!|assert!|expect!\[\[' src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs
```
Record as `AFTER_ASSERTION_CALLS` (now includes `expect!` invocations).

- [ ] **Step 3: Compute deltas**

Using the BASELINE numbers from Task 3:

```
BODY_LINES delta   = AFTER_BODY_LINES - BASELINE_BODY_LINES                  (negative = reduced)
ASSERTION delta    = AFTER_ASSERTION_CALLS - BASELINE_ASSERTION_CALLS
PERCENTAGE change  = (delta / BASELINE) * 100
```

Record the exact numbers and percentages. These are headline metrics for the PR description and the spec § 7 Outcomes.

- [ ] **Step 4: Draft the Outcomes content**

Prepare (in scratchpad — written to file in Task 8) text in this exact shape, ready to paste into the spec § 7:

```markdown
## 7. Outcomes

Pilot completed on 2026-XX-XX (replace with actual date).

**Measurements (`decision_log_tests.rs`, all 6 test bodies):**

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Body lines (between `#[test]` and closing `}`) | <BASELINE_BODY_LINES> | <AFTER_BODY_LINES> | <delta> / <pct>% |
| Assertion calls (`assert_eq!` + `assert!` + `expect![[`) | <BASELINE_ASSERTION_CALLS> | <AFTER_ASSERTION_CALLS> | <delta> / <pct>% |
| `log.contains` probes | <BASELINE_CONTAINS_PROBES> | 3 (test #6 only) | <delta> |
| Assertion strength (tests #1–#5) | Weak (substring probes) | Exact (full-string match) | Strengthened |
| Assertion strength (test #6) | Weak (4 probes) | Exact snapshot + 3 negative invariant probes | Strengthened, intent preserved |

**Tests aborted from conversion:** none (determinism check waived per spec § 4 Step 1; no flakiness observed across the refill + read-back cycle).

**Resolved `expect-test` version:** `<from Task 1 Step 4>`.

**Commits:** `<final commit SHA from Task 9>`.

**Skill file final length:** `<line count of 05-snapshot-testing.md from Task 7>` lines.
```

- [ ] **Step 5: No staged change in this task.**

---

## Task 7: Write `skills/05-testing/05-snapshot-testing.md`

The skill is the long-lived deliverable. Spec § 3 prescribes the exact structure (§1–§5); this task implements it. Target length: 200–280 lines.

**Files:**
- Create: `skills/05-testing/05-snapshot-testing.md`

- [ ] **Step 1: Verify the file does not already exist**

Run: `ls skills/05-testing/05-snapshot-testing.md 2>&1`
Expected: `ls: cannot access ... No such file or directory`. If the file exists, do NOT overwrite — stop and reconcile.

- [ ] **Step 2: Confirm the existing file conventions**

Run: `head -3 skills/05-testing/00-unit-testing.md`
Expected: starts with `# Unit testing` (no YAML frontmatter — frontmatter is reserved for the parent `SKILL.md`). The new skill file follows the same pattern.

- [ ] **Step 3: Create the skill file with the prescribed structure**

Write `skills/05-testing/05-snapshot-testing.md`. Required structure (verbatim section order — section bodies are written by the executor following spec § 3's content prescription):

```markdown
# Snapshot testing

> **What this skill is for:** when "manually building an expected struct then comparing it field by field with `assert_eq!`" has become the dominant volume of a test, snapshot testing collapses the expected-value into a string the tool auto-refills, and a reviewer audits via diff. The goal is **reduced boilerplate without weakening assertion meaning** — never the reverse.
>
> Initial scope: pilot conversion of `decision_log_tests.rs` landed 2026-XX-XX. Widening happens organically as future test authors hit a green-light scenario in §1.
>
> Anchor policy: per `feedback_coverage_no_padding` (D1), do not add or convert tests just to pad coverage. Snapshots that turn precise probes into wallpaper are worse than the original.

## §1 — When to use snapshots, when to refuse (decision tree)

### Green light — convert

Snapshot is the *better* assertion when the output IS a single
human-readable artifact whose diff a reviewer can audit at a glance:

- Structured log lines (e.g. `PluginLog` decision tokens, Prometheus
  text format, error-message templates that compose multiple fields).
- Whole-structure YAML / JSON round-trip serialization where the
  property under test is "the entire output shape, not one field".
- `Debug` pretty-print used purely for regression comparison.

### Red light — refuse

Reverse-reference `skills/05-testing/00-unit-testing.md:173–206`. If a
test matches ANY row in that list, it stays as `assert_eq!`:

1. Data-structure invariants (e.g. LRU underflow, NaN protection)
2. Error message text (Condition `reason` / `message` mapping)
3. Algorithmic correctness (IP CIDR, radix-tree, hostname wildcards)
4. Spec compliance (Gateway API regex-length priority etc.)
5. Wire-format / serialization compatibility (serde `rename_all`)
6. Security boundaries (mTLS `verify_depth`, cipher allowlist, SAN logic)
7. Version compatibility (CRD `v1alpha3 → v1`)
8. Regression fixes with historical bug IDs (`P0-07` etc.)
9. Pure cryptography / JWT issuance and verification

Named repo examples:

| Category | Example |
|----------|---------|
| Algorithmic correctness | `src/types/resources/tests/edgion_tls_test.rs` hostname wildcard tests |
| Data-structure invariants | `src/core/gateway/cache/lru.rs` underflow / hit_ratio |
| Security boundaries | `src/core/gateway/tls/validation/cert.rs` mTLS verify_depth |
| Wire-format | `src/core/common/local_auth/handlers.rs` AuthStatus JSON shape |
| Regression fix | `src/core/gateway/lb/runtime_state/mod.rs` P0-07 leastconn |

### Yellow light — usually refuse, with a worked counter-example

`src/types/resources/link_sys/tests/webhook_test.rs::test_default_config`
(lines 5–18) is the canonical anti-pattern. It checks ~10 mixed
assertions over independent default-value fields — 5 × `assert!`, 4 ×
`assert_eq!`, 1 × `matches!` pattern — each asserting a different field
of the default config.

If you collapsed them into one `expect![[...]]` block you would:

- **Lose field-by-field intent** — a reviewer no longer sees "we care
  that `timeout_ms == 5000`"; they see a string blob.
- **Couple unrelated changes** — adding any new field to the struct
  forces a snapshot refill for all unrelated tests.
- **Erase the `matches!` semantics** — `matches!(target, WebhookTarget::Uri { uri })`
  is a structural assertion; a string snapshot cannot express that.

Bad rewrite (illustration only — DO NOT do this):

\```rust
// Anti-pattern: independent-field defaults collapsed into a snapshot
#[test]
fn test_default_config_BAD() {
    let config = WebhookServiceConfig::default();
    let dump = format!("{config:#?}");
    expect![[r#"... 30+ lines of Debug output ..."#]].assert_eq(&dump);
}
\```

Original is better. Keep it as `assert!` / `assert_eq!` per field.

## §2 — How to write expect-test (operations manual)

Add `expect-test = "1.5"` (or current pinned version) to
`Cargo.toml`'s `[dev-dependencies]`. Import as:

\```rust
use expect_test::expect;
\```

### Three templates

**Single-line string** (decision log, status header):

\```rust
expect![[r#"OK; "#]].assert_eq(&render(&input));
\```

**Multi-line structure** (YAML round-trip):

\```rust
expect![[r#"
apiVersion: edgion.io/v1
kind: EdgionTls
metadata:
  name: example
"#]].assert_eq(&serde_yaml::to_string(&value).unwrap());
\```

**Large `Debug` dump** (move out of line):

\```rust
expect_file!["./snapshots/large_output.txt"]
    .assert_eq(&format!("{value:#?}"));
\```

### Refill workflow

\```bash
# Default form — inline test modules (which is how Edgion organizes tests).
# `-p <package>` selects the crate (single-crate repo: -p edgion).
# The trailing arg is a test-name filter, typically a module path.
UPDATE_EXPECT=1 cargo test -p edgion <module::path::filter>
git diff   # MANDATORY — human eyes review every refilled snapshot
# only commit after verifying every diff reflects intended behavior change
\```

`cargo test --test <suite>` is only correct for integration test
binaries under a top-level `tests/` directory (Edgion has none as of
2026-05-27 — all tests are inline `#[cfg(test)] mod tests`). Use the
module-path filter form.

### Hard rule — own paragraph, do not skim

> **Never `UPDATE_EXPECT=1` without reading the diff.** A failing
> snapshot is a question the test is asking you: "did this output
> change on purpose?" Auto-accepting collapses that question into
> silence and turns snapshots into a coverage-shaped no-op. If a PR
> ever lands a snapshot refill without a reviewer-visible
> justification in the diff, that's a process failure, not a
> tooling success.

Anti-pattern: a PR titled "fix flaky test" that contains only
`UPDATE_EXPECT=1` output and no behavioral change. Almost always
a silenced regression.

## §3 — Volatile fields (escape hatches, in priority order)

When the output contains timestamps, generated IDs, memory addresses,
or other non-deterministic substrings, try these in order; do NOT skip
ahead.

### When the probe is needed at all

The probe is conditional: it is for inputs that **plausibly involve
non-determinism** — clocks, RNG, allocator addresses, async ordering,
OS-dependent enumeration. For inputs that are literal scripts + mocked
sessions + fixed enums + in-process buffers (e.g. `decision_log_tests.rs`
in the B1 pilot), the probe is waived with a one-sentence justification.
Do not perform it ritually.

### Probe (when needed)

Run the suite **at least five times** with the actual snapshot input
dumped to stdout. `diff` the outputs. Any divergence localizes the
volatile substring.

### Fix order

- **(a) Inject determinism (preferred)** — make the function under
  test accept an injected clock / RNG / ID generator. This removes
  volatility at the source.
- **(b) Sanitize before comparing** — wrap actual output through a
  small local `sanitize()` helper that replaces volatile substrings
  with stable placeholders. Prefer `str::replace` / `split.join` over
  pulling in a regex crate for trivial cases.

  \```rust
  fn sanitize(s: &str) -> String {
      s.replace(char::is_numeric, "<N>")
  }
  expect![[r#"ts=<N><N><N>"#]].assert_eq(&sanitize(&actual));
  \```

- **(c) Switch to `insta` with redaction** — only when (a) is
  structurally blocked AND (b) requires a regex zoo larger than the
  test itself. `insta` is NOT a project dependency today (2026-05-27);
  introducing it requires:
  - Adding `insta = "1"` to `[dev-dependencies]`
  - Documenting the choice in the PR description with a worked example
    that demonstrates (a) and (b) were considered and rejected
  - Optionally adding `cargo insta review` to a Makefile target

  Do not jump to (c) without exhausting (a) and (b).

## §4 — Quantification rubric

Report four numbers, computed only over the **subset of tests
actually converted** (never the whole file — whole-file averages
dilute the signal):

| Metric | What to count |
|--------|---------------|
| Body lines, before / after | Lines between `#[test]` and the closing `}` of each converted `fn` |
| Assertion calls, before / after | `assert!` + `assert_eq!` + `expect![[` invocations |
| Assertion strength delta | Qualitative: "strengthened" (weak `contains` probes → exact match), "equivalent" (multiple `assert_eq!` collapsed without semantic loss), "weakened" (must NOT occur — abort the conversion) |
| Maintenance cost delta | Qualitative: "lower" (one-command refill replaces N manual edits), "neutral", "higher" (rare — usually means a yellow-light case slipped through) |

A "weakened" result is a hard abort. Revert the conversion and
document it as a worked anti-example here.

## §5 — Registration

This skill is registered in `skills/05-testing/SKILL.md` Part 1 —
Standard tests. When updating it, run `make check-agent-docs` to
confirm the registry is consistent.
```

The above content is a complete prescription. When implementing, you may rephrase the prose for flow but **do not drop sections, do not drop the named examples, do not soften the hard-rule callout, do not change the priority order in §3**.

- [ ] **Step 4: Verify length is within target (200–280 lines)**

Run: `wc -l skills/05-testing/05-snapshot-testing.md`
Expected: a number between 200 and 280. If the file is much shorter, you have probably skipped a section's content; if much longer, you have probably added prose that the prescription does not require.

- [ ] **Step 5: Verify the "Initial scope" line includes the actual pilot date**

Replace `2026-XX-XX` in the preamble (step 3) with the date you complete this task (e.g. `2026-05-27`).

- [ ] **Step 6: Stage**

```bash
git add skills/05-testing/05-snapshot-testing.md
```

Do NOT commit.

---

## Task 8: Register the skill + append Outcomes + run `check-agent-docs`

**Files:**
- Modify: `skills/05-testing/SKILL.md` (Part 1 table)
- Modify: `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md` (append to § 7)

- [ ] **Step 1: Add the new skill to `SKILL.md` Part 1 — Standard tests table**

Read `skills/05-testing/SKILL.md` lines 13–19. Below the existing rows (after `02-k8s-integration-testing.md`), add ONE row:

```markdown
| [05-snapshot-testing.md](05-snapshot-testing.md) | Snapshot testing: when to use / refuse, expect-test workflow, volatile-field escape hatches | Complete |
```

Match the existing column ordering and styling exactly. Use `Complete` (single-word, matching existing vocabulary at lines 17–19) — do NOT use `Complete (pilot)`; the pilot framing already lives in the skill file's preamble per Task 7.

- [ ] **Step 2: Replace the spec's § 7 Outcomes placeholder with the real measurements**

Read `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md` § 7 (the section currently titled "Outcomes (filled in after execution)"). Replace its body with the content prepared in Task 6 Step 4, filling in all `<...>` placeholders with the actual numbers and date.

After this edit, § 7 contains the final measurement table, the resolved `expect-test` version, the skill file's final line count (from Task 7 Step 4), and a note "Tests aborted from conversion: none" (unless something forced an abort during Tasks 4–5 — in which case document why).

- [ ] **Step 3: Run the doc-consistency check**

Run: `make check-agent-docs 2>&1 | tail -30`
Expected: exit 0, output ends with success message. If the validator complains about the new row in `SKILL.md` (column count, formatting, or path validation), fix the row and re-run.

If `check-agent-docs` fails for unrelated reasons (pre-existing skill-doc drift you did not touch), report it but do NOT try to fix unrelated issues — that's a separate todo.

- [ ] **Step 4: Stage**

```bash
git add skills/05-testing/SKILL.md docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md
```

Do NOT commit.

---

## Task 9: Main-session final verification + commit handoff

This task runs the checks the spec § 4 Step 4 reserves for the main session (NOT the subagent). It also produces the final commit handoff.

**Files:**
- Read all staged files
- No new modifications expected (if any verification fails, this task may produce small fixes)

- [ ] **Step 1: Confirm what is staged**

Run: `git status --short`
Expected: a set of `M` / `A` lines covering all files Tasks 1–8 staged. Specifically:
- `Cargo.toml`
- `Cargo.lock`
- `src/core/gateway/plugins/runtime/log.rs`
- `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs`
- `skills/05-testing/05-snapshot-testing.md` (new file)
- `skills/05-testing/SKILL.md`
- `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md`

If something is missing, go back and stage it.

- [ ] **Step 2: Run clippy on all targets (main-session-only check)**

Run: `cargo clippy --all-targets 2>&1 | tail -30`
Expected: no errors. Warnings introduced by Tasks 2 or 4–5 must be addressed (e.g. unused imports, unnecessary clones). Pre-existing warnings are fine.

If clippy flags `render_for_test` for anything (dead code under non-test config, unused, etc.), it usually means the `#[cfg(test)]` gate is missing or wrong; re-check Task 2 Step 3.

- [ ] **Step 3: Run the full unit-test suite for the affected crate**

Run: `cargo test -p edgion --lib --tests 2>&1 | tail -20`
Expected: all tests pass. This is the broader sanity check that the targeted runs in earlier tasks may have missed.

If any unrelated test broke, investigate — it usually means the new dev-dep `expect-test` caused a feature-resolution interaction. If a test that was already broken before this work fails, document it but do not try to fix it (separate todo).

- [ ] **Step 4: Re-run the doc consistency check (it may depend on file-tree state)**

Run: `make check-agent-docs 2>&1 | tail -10`
Expected: exit 0.

- [ ] **Step 5: Read all staged diffs one final time**

Run: `git diff --cached`
Expected: every change is intentional. In particular re-verify:
- Every `expect![[r#"..."#]]` block contains plausible decision-log content, never a stack trace, address, or other accident.
- Test #6 retains its three negative `assert!` invariant probes after the `expect!`.
- `skills/05-testing/05-snapshot-testing.md` matches the structure prescribed in Task 7.
- `skills/05-testing/SKILL.md` row uses `Complete`, not `Complete (pilot)`.
- The spec's § 7 has real numbers, not placeholders.

- [ ] **Step 6: Hand off to the user for commit approval**

Per `feedback_test_cover_execution` and the spec § 4 Step 4: subagents and the implementer do NOT commit. Report this back to the user:

> Pilot complete. All eight stages done, all checks green:
> - `cargo fmt`, `cargo check --lib --tests`, `cargo clippy --all-targets`, `cargo test -p edgion --lib --tests`, `make check-agent-docs` — all pass.
> - decision_log_tests.rs: 6/6 passing; 5 fully converted, test #6 partial.
> - `expect-test = <version>` added as the only new dev-dependency.
> - Skill file at `skills/05-testing/05-snapshot-testing.md` (<line count> lines).
> - Spec § 7 Outcomes populated with: body lines <before>→<after> (<pct>%), assertion calls <before>→<after> (<pct>%).
>
> Ready to commit. Suggested message:
>
> ```
> feat(test): snapshot-testing skill + B1 pilot in decision_log_tests
>
> Add expect-test-based snapshot rubric (skills/05-testing/05-snapshot-testing.md)
> and convert 5 of 6 decision_log_tests.rs tests fully to expect-test, with
> test #6 retaining its negative-invariant probes alongside the snapshot.
>
> - PluginLog::render_for_test (#[cfg(test)]) is the snapshot input;
>   \n separates entries inside a tier, \n--- \n separates tier-1 from tier-2
> - expect-test = "1.5" added as dev-dependency (resolved <version>)
> - Body lines: <before> → <after> (<pct>%); assertion calls: <before> → <after> (<pct>%)
> - Assertion strength: weak `contains` probes → exact-string match
>   (test #6 also retains invariant-explanation `!contains` lines)
> - Skill registered in skills/05-testing/SKILL.md as Complete
>
> Implements: docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md
>
> Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
> ```
>
> Awaiting explicit "commit" instruction.

- [ ] **Step 7: Wait for user instruction before committing**

Do NOT auto-commit. The project CLAUDE.md is explicit: `Do not run git commit or git push on your own`.

---

## Out of scope (do NOT do as part of this plan)

These are explicitly NOT part of this work, per the spec's `feedback-no-over-engineering` alignment:

- **Adding `insta` to dependencies.** The skill documents it as an escape hatch; introducing the crate is a separate todo when a real volatile-field case demands it.
- **Pre-listing other test files to convert.** The skill's §1 decision tree IS the rollout mechanism. Do not scan the repo for "candidate files".
- **Modifying PR templates** to enforce the "review every refilled snapshot" rule. Future todo.
- **Converting unrelated test files** in the same commit. Touch only `decision_log_tests.rs`.
- **Refactoring `PluginLog` itself.** Add only `render_for_test`; leave the rest untouched.
- **Running `cargo test --all`** as part of subagent verification. Targeted tests only; `--all` is a main-session decision if anything (Task 9 Step 3 already runs `-p edgion --lib --tests`, which is the practical equivalent for this single-crate project).
