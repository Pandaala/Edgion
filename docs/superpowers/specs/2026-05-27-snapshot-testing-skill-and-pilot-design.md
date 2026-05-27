# Snapshot Testing Skill + B1 Pilot — Design

> Source: `tasks/todo/snapshot-testing-skill-and-b1.md` (B1 carved out of
> `tasks/todo/test-organization-and-review-cost.md`). This document is the
> brainstormed-and-approved design; the next step is `writing-plans`.

| Key | Value |
|-----|-------|
| Created | 2026-05-27 |
| Status | Design approved — ready for writing-plans |
| Type | skill + refactor(test) |
| Priority | P2 |
| Owner | Edgion test platform |

## 1. Goal

Two deliverables, each independently verifiable:

1. **`skills/05-testing/05-snapshot-testing.md`** — a long-lived authoritative
   skill that combines a **decision rubric** (when to use snapshots, when to
   refuse) with an **operations manual** (how to write / refill / handle
   volatile fields).
2. **B1 pilot: convert `decision_log_tests.rs` to expect-test** — quantify the
   change in line count, assertion count, and assertion strength so the
   organization can later decide whether and where to widen adoption.

Both deliverables must respect the project's "no padding for coverage" stance
(`feedback_coverage_no_padding`): snapshot testing must **never weaken
assertion semantics** — the gain is reduced boilerplate plus, where it
applies, *stronger* assertions (exact-string match beats multiple `contains`
probes), never weaker.

## 2. Architecture and dependencies

### 2.1 Files touched

| File | Change |
|------|--------|
| `skills/05-testing/05-snapshot-testing.md` | New, 200–280 lines |
| `skills/05-testing/SKILL.md` | Add one row in Part 1 — Standard tests |
| `Cargo.toml` `[dev-dependencies]` (line 202, single-crate project — no workspace) | Add `expect-test = "1.5"` |
| `src/core/gateway/plugins/runtime/log.rs` | Add `#[cfg(test)] pub fn render_for_test(&self) -> String` on `PluginLog` (see §2.2) |
| `src/core/gateway/plugins/http/dsl/tests/decision_log_tests.rs` | Rewrite 5 of 6 test bodies; test #6 partial conversion (see §4 Step 2 / I1) |
| `docs/superpowers/specs/2026-05-27-snapshot-testing-skill-and-pilot-design.md` | This document |

### 2.2 PluginLog rendering helper (prerequisite for the pilot)

`PluginLog` (in `src/core/gateway/plugins/runtime/log.rs`) does **not**
implement `Display` and has no public `to_string` / `as_str`. The buffers
expose `entries() -> Iterator<Item = &str>` only. Before any test can use
`expect![[...]].assert_eq(&log_string)`, a renderer must exist.

**Decision (load-bearing — this string format enters every snapshot):**

```rust
// src/core/gateway/plugins/runtime/log.rs
#[cfg(test)]
impl PluginLog {
    pub fn render_for_test(&self) -> String {
        let main: Vec<&str> = self.log.iter().flat_map(|b| b.entries()).collect();
        let user: Vec<&str> = self.ulog.iter().flat_map(|b| b.entries()).collect();
        match (main.is_empty(), user.is_empty()) {
            (true, true) => String::new(),
            (false, true) => main.join("\n"),
            (true, false) => user.join("\n"),
            (false, false) => format!("{}\n--- \n{}", main.join("\n"), user.join("\n")),
        }
    }
}
```

- Separator inside one buffer: `\n` (entries are already individually
  meaningful).
- Separator between main `log` and user `ulog`: literal `\n--- \n` — a
  marker reviewers can grep for, visually obvious in `git diff`.
- `#[cfg(test)]` keeps this out of production binaries; chosen explicitly
  to avoid a `Display` impl on `PluginLog` that could leak into a
  production `println!`.

If a future case needs richer rendering (per-field colors, timestamps,
etc.) it adds a sibling `render_*_for_test` method — do not overload
`render_for_test`'s contract.

### 2.3 Dependencies — explicit non-choices

- **`insta` is NOT introduced.** The skill documents `insta` as the escape
  hatch for "output too large to read inline" or "volatile fields needing
  redaction", but no crate is added until a real case forces it. Adding two
  snapshot crates up front would split the mental model and dilute the
  pilot's signal.
- **No PR template changes** in this todo. The skill's "human eyes review
  every diff before `UPDATE_EXPECT=1`" rule could one day be enforced via a
  PR-template checkbox; that lands in a future todo, not here
  (`feedback-no-over-engineering` — document the risk, do not pre-build the
  guardrail).
- **No rollout candidate list.** The skill's decision tree is the rollout
  mechanism: any future test author hitting a green-light scenario picks up
  the skill and applies it. Pre-scanning the repo and listing "files to
  convert" would re-introduce the padding mindset D1 explicitly bans.

### 2.4 Cross-references to anchor against drift

- `skills/05-testing/00-unit-testing.md:173–206` — the "must-keep-as-unit-test"
  list. The new skill's red-light section **reverse-references this list in
  full**: any test matching items 1–9 (algorithmic correctness, data-structure
  invariants, error message text, security boundaries, serialization
  compatibility, version compatibility, regression fixes, cryptography/JWT)
  **must not** be converted.
- `feedback_coverage_no_padding` — quoted in the skill's opening paragraph.

## 3. Skill content outline (`05-snapshot-testing.md`)

The skill is the long-lived deliverable; the pilot exists to validate it.
Five sections, decision-first.

### §1 — When to use snapshots, when to refuse (decision tree)

**Green light — convert (snapshot is the *better* assertion):**

- String / byte rendering output: structured log lines (PluginLog decision
  tokens, Prometheus text format, error-message templates that compose
  multiple fields).
- Whole-structure YAML / JSON round-trip serialization where the property
  under test is "the entire output shape, not one field".
- `Debug` pretty-print used purely for regression comparison.

Common trait: **the output is a single human-readable artifact whose diff
a reviewer can audit at a glance**.

**Red light — refuse, reverse-reference `00-unit-testing.md:173`:**

The skill quotes that file's list verbatim and adds: *if a test matches any
row, do not convert.* Examples explicitly named:

| Category | Example test in the repo |
|---|---|
| Algorithmic correctness | `edgion_tls_test.rs` hostname wildcard matching (~80 tests) |
| Data-structure invariants | `lru.rs` underflow / hit_ratio NaN protection |
| Security boundaries | `cert.rs` mTLS verify_depth, SAN coverage |
| Error message text | `status_utils.rs` Condition reason/message mapping |
| Wire-format compatibility | `local_auth/handlers.rs` AuthStatus JSON shape |

**Yellow light — usually refuse, with a worked counter-example:**

`webhook_test.rs::test_default_config` (lines 5–18) is the canonical
anti-pattern. It checks ~10 mixed assertions over independent default-value
fields — 5 × `assert!`, 4 × `assert_eq!`, 1 × `matches!` pattern — each
asserting a different field of the default config.

Merging them into one `expect![[r#"..."#]]` block:

- **Loses field-by-field intent** — a reviewer reading the test no longer
  sees "we care that `timeout_ms == 5000`"; they see a string blob.
- **Couples unrelated changes** — adding a new field to the struct forces
  a refill that touches the snapshot for unrelated tests.
- **Erases the `matches!` pattern semantics** — the original `matches!`
  on `WebhookTarget::Uri { uri }` is a structural assertion, not a value
  one; a serialized snapshot cannot express that distinction.

The skill embeds a code-vs-code before/after comparison so the lesson is
concrete.

### §2 — How to write expect-test (operations manual)

**Base form:**

```rust
use expect_test::expect;

let actual = render(&input);
expect![[r#"OK; "#]].assert_eq(&actual);
```

**Three templates, one per shape:**

- *Single-line string* (decision log) — inline `expect![[r#"..."#]]`.
- *Multi-line structure* (YAML round-trip) — `expect![[r#"..."#]]` with
  a multi-line string literal; indentation matters.
- *Large `Debug` dump* — `expect_file!["./snapshots/foo.txt"]` when inline
  would dominate the test file.

**Refill workflow:**

```bash
# Default form — inline test modules (which is how Edgion organizes tests).
# `-p <package>` selects the crate (single-crate repo: -p edgion).
# The trailing arg is a test-name filter, typically a module path.
UPDATE_EXPECT=1 cargo test -p edgion <module::path::filter>
git diff   # MANDATORY — human eyes review every refilled snapshot
# only commit after verifying every diff reflects intended behavior change
```

`cargo test --test <suite>` is only correct when the test is a true
integration-test binary under a top-level `tests/` directory (Edgion
has none under that pattern as of 2026-05-27 — all unit tests are
inline `#[cfg(test)] mod tests`). Use the module-path filter form.

**Hard rule (callout, own paragraph):**

> **Never `UPDATE_EXPECT=1` without reviewing the diff.** A failing
> snapshot is a question the test is asking you: "did this output change
> on purpose?" Auto-accepting collapses that question into silence and
> turns snapshots into a coverage-shaped no-op. If a PR ever lands a
> snapshot refill without a reviewer-visible justification in the diff,
> that's a process failure, not a tooling success.

A short anti-example illustrates a PR that mechanically refills and ships
a regression.

### §3 — Volatile fields (escape hatches, in priority order)

When the output contains timestamps, generated IDs, memory addresses, or
other non-deterministic substrings, try these in order; do not skip ahead:

- **(a) Root-cause it** — make the function under test accept an injected
  clock / RNG / ID generator. This is almost always achievable for unit
  tests and removes the volatility instead of papering over it.
- **(b) Sanitize before comparing** — wrap the actual output through a
  small `sanitize()` helper that replaces volatile substrings with
  stable placeholders (e.g., `Regex::replace("ts=[0-9]+", "ts=<TS>")`).
  Keep the helper in the test module so its intent is local.
- **(c) Switch to `insta` with redaction** — only when (a) is structurally
  blocked (e.g., the output is bytes from a foreign serializer) and (b)
  would require a regex zoo larger than the test itself.

Path (c) is **not exercised by the B1 pilot**. The skill documents the
crate name, redaction syntax, and "if you add it, also update
`Makefile`/CI" checklist, but the dependency is not committed until a real
case demands it.

### §4 — Quantification rubric (shared by pilot and future adopters)

Report four numbers, computed only over the **subset of tests actually
converted**, never over the whole file (whole-file averages dilute signal):

| Metric | What to count |
|---|---|
| **Lines (test bodies, before/after)** | Body lines of the converted `fn`s only |
| **Assertion count (before/after)** | Total `assert*` / `expect*` invocations |
| **Assertion strength delta** | Qualitative: "strengthened" (e.g., `contains` weak probes → exact match), "equivalent" (multiple `assert_eq!` collapsed into one `expect!` with same semantic coverage), or "weakened" (must not occur — abort conversion) |
| **Maintenance cost delta** | Qualitative: "lower" (one-command refill replaces N manual edits), "neutral", "higher" (rare; usually means the test was a yellow-light case) |

If any conversion produces "weakened", that conversion is reverted and
recorded as a skill anti-example.

### §5 — Registry obligations

- Add a row to `skills/05-testing/SKILL.md` Part 1 — Standard tests table.
  Use `Complete` to match existing Status vocabulary (single-word: existing
  values are `Complete` and `Refactored`; introducing `Complete (pilot)`
  would be a one-off vocabulary divergence that the doc validator may or
  may not accept). The "pilot" framing belongs inside the skill file's
  opening preamble paragraph, not in the SKILL.md table:

  | File | Topic | Status |
  |------|-------|--------|
  | `05-snapshot-testing.md` | Snapshot testing: when to use / refuse, expect-test workflow, volatile-field escape hatches | Complete |

- Skill file's opening line includes: *"Initial scope: pilot conversion
  of `decision_log_tests.rs` (2026-05-27). Widening happens organically
  as future test authors hit a green-light scenario."*

- Run `make check-agent-docs` and confirm pass.

## 4. B1 pilot — workflow

### Step 1: Determinism check (conditional, NOT a ritual)

The skill §3 prescribes a divergence probe only when the input shape
**plausibly involves non-determinism** — clocks, RNG, allocator
addresses, async ordering, OS-dependent enumeration, etc. For this
specific file the inputs are literal scripts, mocked sessions, fixed
policy enums, and a `PluginLog::new("test")` with no clock injection
(see `mod.rs:128–147`). A 5-run probe would trivially pass and
provide zero signal.

**Therefore the pilot skips the probe and records the justification:**

> "Inputs are deterministic: literal DSL scripts → mocked session →
> fixed `DslErrorPolicy::Deny` → in-process `PluginLog`. No clock,
> RNG, async, or OS-dependent enumeration. Probe waived per skill §3."

The skill writes the conditional rule explicitly so future converters
neither skip it silently nor perform it ritually. If during Step 2 a
refilled snapshot turns out flaky in CI, **fall back** to the
divergence probe to localize which substring is volatile, then apply
skill §3 (a)→(b)→(abort) in order. Do not jump to `insta` for the
pilot.

If a test is aborted (kept in its original `assert_eq` / `contains`
form), exclude it from the conversion count and record the reason as a
worked anti-example in the skill. That outcome is a legitimate pilot
result — the decision tree functioning, not a failure.

### Step 2: Rewrite the test bodies

**5 of 6 tests (#1–#5): full conversion.**

- **Keep verbatim:** `assert_eq!(result, <enum>)` — the result enum
  comparison is semantic (decision type / status code / gRPC code), a red-
  light case (algorithmic / data-structure correctness).
- **Replace as a group:** all `log.contains(...)` plus `!log.contains(...)`
  probes within one test → one
  `expect![[r#"<full expected log>"#]].assert_eq(&log.render_for_test())`.
- This strengthens assertions: previously `OK; foo dsl match bar` could
  pass the `contains("OK; ")` + `!contains("dsl match")` pair only if the
  literal substring "dsl match" did not appear; the new exact match
  forbids any drift.

**Test #6 (`test_decision_log_runtime_error_emits_only_err_entry`):
PARTIAL conversion.**

Per the I1 finding from review: this test carries **four** distinct
invariants in its probes — one positive (`contains("err: ")` — error
path DOES emit `err: `) and three negative (`!contains("OK")`,
`!contains("Deny")`, `!contains("dsl match")` — error path does NOT
emit any decision token or stale marker). Collapsing them all into a
single `expect!` block makes
the exact-string assertion technically stronger but **loses the
invariant explanations** (the strings `"error path must not emit 'OK'"`
in the `assert!` messages). A future regression that re-introduces an
`OK` token inside an error path would fail the snapshot with no
human-readable "why does this matter".

For test #6 only, **keep** the negative `!contains(...)` probes AND
**add** the `expect!` snapshot — both coexist. The snapshot guards
exact format; the negative probes carry semantic-intent messages a
reviewer can read in a failure output. This test counts as "partial
conversion" in the quantification table.

**Refill workflow (all 6 tests):** run

```bash
UPDATE_EXPECT=1 cargo test -p edgion dsl::tests::decision_log_tests
```

then `git diff` and **read every refilled string** before accepting.
(Note: this is a **module-path test filter** against the single
`edgion` package — NOT `cargo test --test <suite>`, because these
tests are inline modules included via `tests/mod.rs`, not integration
test binaries.)

### Step 3: Quantification report (recorded in PR description + spec Outcomes)

Computed only over the 6 tests in `decision_log_tests.rs` (5 full + 1
partial per Step 2):

| Metric | Before (measured) | After (measured) | Delta |
|---|---|---|---|
| Lines (test bodies, lines 10–81) | ~70 | TBD | TBD |
| Assertion count | 6 × `assert_eq!` + ~22 × `contains` ≈ 28 | 6 × `assert_eq!` + 5 × `expect!` + (test #6: 1 × `expect!` + 4 × `contains` retained) ≈ 16 | ≈ −43% |
| Assertion strength (tests #1–#5) | Weak (substring probes) | Exact (full-string match) | **Strengthened** |
| Assertion strength (test #6) | Weak + invariant messages | Exact + invariant messages retained | **Strengthened, intent preserved** |
| Maintenance cost | Log format change requires N manual edits | `UPDATE_EXPECT=1` one-shot refill | Lower |

The "Before" `~70` and ≈ counts are estimates; the executor measures
and fills exact numbers when the pilot completes.

### Step 4: Verify (subagent + main-session split)

Per `feedback_test_cover_execution` (specific coding dispatched to
sonnet subagent) and `feedback_no_clippy_no_build` (subagent verify scope
is narrow), the **subagent runs only:**

```bash
cargo fmt --all
cargo check --lib --tests
cargo test -p edgion dsl::tests::decision_log_tests
```

(Single-crate project — no `-p <package>` ambiguity; the `-p edgion`
above is for the test filter form, not a workspace selector.)

The subagent does **not** run `cargo clippy`, does **not** run
`cargo build` for the workspace, does **not** run `cargo test --all`.

**Main session, before commit handoff, additionally runs:**

```bash
cargo clippy --all-targets
make check-agent-docs
```

These broader checks close the loop: clippy catches any lint regression
in the edited files (`render_for_test`, `decision_log_tests.rs`), and
`check-agent-docs` confirms the new `05-snapshot-testing.md` is properly
registered in `skills/05-testing/SKILL.md`.

**Nothing is committed by the subagent.** The main session collects all
changes and commits once at the end on explicit user instruction.

### Step 5: Rollout decision

**There is no scheduled rollout phase.** The skill's §1 decision tree is
the rollout mechanism: each future test author hitting a green-light
scenario picks the skill up themselves. This is consistent with D1 — no
pre-listed quotas, no padding incentive, no "we said we'd convert N files"
pressure.

## 5. Risks and contingencies

### R1 — Output is volatile, expect-test goes flaky

For this pilot the determinism check is **waived** (Step 1; inputs are
literal scripts + mocked session + fixed policy, no clock/RNG/async).
The fallback is: if a refilled snapshot turns out flaky in CI, **then**
run the conditional divergence probe from skill §3 to localize the
volatile substring, and apply (a) inject determinism → (b) sanitize →
(abort) in order. Worst case: that test is aborted (left as
`assert_eq` / `contains`), the file is documented as a "decision tree
correctly refused this conversion" example, and the skill still ships
intact. A bad conversion prevented is a positive pilot outcome, not
a failure.

### R2 — Future readers misapply the skill to single-field-dense files

Mitigated by the §1 yellow-light worked counter-example using
`webhook_test.rs::test_default_config`. A concrete reference is harder to
misread than an abstract rule.

### R3 — `UPDATE_EXPECT=1` rot ("red? refill")

Mitigated by §2's hard-rule callout, the anti-example, and the
quantification rubric's "strengthened / equivalent / weakened" axis (a
weakened result aborts the conversion, making the bad path visible). PR
template enforcement is logged as future work — not part of this todo per
`feedback-no-over-engineering`.

### R4 — `expect-test` dependency side effects / version drift

Mitigated by dev-dependency isolation (no release-build impact); the
`Cargo.lock` diff is reviewable in the PR. Pinning to `expect-test = "1.5"`
(matches the existing `mockall = "0.14.0"` / `proptest = "1"` minor-pin
style in `Cargo.toml:202-218`) keeps the resolver from silently pulling
a future breaking change inside major version 1; the executor verifies
the latest published 1.x at implementation time and records the resolved
version in the PR description.

## 6. Alignment with existing policies

| Policy | How this design honors it |
|---|---|
| `feedback_coverage_no_padding` (D1) | Pilot strengthens assertion semantics; skill §1 opens with "snapshots must not weaken assertion meaning"; no pre-listed rollout quota |
| `feedback_test_cover_execution` | Step 4 dispatches coding to sonnet subagent; main session does not commit mid-flow |
| `feedback_no_clippy_no_build` | Subagent verify scope = fmt + targeted check + targeted test; no clippy, no full build/test. Main session runs clippy + check-agent-docs before commit handoff (Step 4) — closes the loop without violating the subagent-scope rule |
| `feedback-no-over-engineering` | `insta` left as documented escape hatch, not adopted; PR-template enforcement deferred; no rollout candidate list |
| `00-unit-testing.md:173–206` | Skill §1 red-light section reverse-references this list verbatim, with named repo examples per row |
| English-only on disk | Skill content, code comments, commit messages all in English |

## 7. Outcomes

Pilot completed on 2026-05-27.

**Measurements (`decision_log_tests.rs`, all 6 test bodies):**

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Body lines (between `#[test]` and closing `}`) | 67 | 71 | +4 / +6% |
| Assertion calls (`assert_eq!` + `assert!` + `expect!`) | 20 | 15 | -5 / -25% |
| `log.contains` probes | 14 | 3 (test #6 only) | -11 |
| Assertion strength (tests #1–#5) | Weak (substring probes) | Exact (full-string match) | Strengthened |
| Assertion strength (test #6) | Weak (4 probes) | Exact snapshot + 3 negative invariant probes | Strengthened, intent preserved |

**Tests aborted from conversion:** none (determinism check waived per spec § 4 Step 1; no flakiness observed across the refill + read-back cycle).

**Resolved `expect-test` version:** `1.5.1`.

**Commits:** `<final commit SHA from Task 9>` (filled at commit time).

**Skill file final length:** 280 lines.
