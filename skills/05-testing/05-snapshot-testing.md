# Snapshot Testing with `expect-test`

Snapshot testing captures a function's complete output as a string and fails when that output changes unexpectedly.
The goal is reduced boilerplate **without weakening assertion meaning** — never the reverse.
A snapshot that turns a precise behavioral probe into an unread wall of text is worse than the original `assert_eq!`.

**Pilot scope.** Initial conversion of `decision_log_tests.rs` landed 2026-05-27.
Widening happens organically as future test authors encounter a green-light scenario in §1 below.

**Honest pilot result.** Body line count went **UP** (+4 / +6%), not down.
The headline wins were assertion-call count (−25%) and `log.contains` probes (14 → 3), plus a qualitative
upgrade from weak substring matches to exact full-string assertions.
Snapshot testing is **not** primarily a line-reduction tool; its value is assertion-strength and
maintenance-cost. Future output changes surface as one reviewable diff rather than a scatter of grep failures.

**Anchor policy (`feedback_coverage_no_padding`, D1).** Do not convert tests just to look productive.
Only convert when the test falls into a green-light scenario below.

---

## §1 — When to use snapshots, when to refuse

### Green light — convert

The output is a **single human-readable artifact** whose diff a reviewer can audit at a glance.

| Scenario | Example in repo |
|----------|-----------------|
| Structured log lines (PluginLog decision tokens, Prometheus text format) | `decision_log_tests.rs` (B1 pilot) |
| Whole-structure YAML / JSON round-trip (serialize → deserialize → re-serialize) | Any CRD round-trip test that currently diffs a large `assert_eq!(yaml, expected_yaml)` block |
| `Debug` pretty-print for regression comparison | Any `#[derive(Debug)]` type whose formatted output is the contract |

### Red light — refuse

The following categories **must never be converted to snapshots**.
They are drawn from `skills/05-testing/00-unit-testing.md:173–206`.
A snapshot cannot express the invariant being checked in these cases — it can only obscure it.

| Category | Named repo example |
|----------|--------------------|
| Data-structure invariants | `edgion-algo/src/cache/lru.rs` — LRU underflow / hit-ratio NaN |
| Error message text | `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/status_utils.rs` Condition mapping |
| Algorithmic correctness | `edgion-resources/src/resources/tests/edgion_tls_test.rs` hostname wildcard tests |
| Spec compliance | `edgion-gateway/src/routes/grpc/match_engine.rs` sort tests |
| Wire-format / serialization compatibility | `edgion-resources/src/auth_dto.rs` `AuthStatusResponse` JSON wire format |
| Security boundaries | `edgion-gateway/src/tls/validation/cert.rs` mTLS `verify_depth` / SAN coverage |
| Version compatibility | `edgion-controller/src/conf_mgr/conf_center/file_system/resource_controller.rs` v1alpha3→v1 |
| Regression fixes with bug IDs | `edgion-gateway/src/lb/runtime_state/mod.rs` P0-07 leastconn count |
| Pure cryptography / JWT verification (gateway OIDC plugin) | `edgion-gateway/src/plugins/http/openid_connect/jwt_common.rs` algorithm policy / JWK selection |

### Yellow light — usually refuse

Yellow-light tests mix several distinct assertion kinds in a single body.
Merging them into one `expect!` block loses field-by-field intent and couples unrelated behavioral changes
(adding a new field forces a refill even when the new field is irrelevant to the test's concern).

**Worked counter-example:** `edgion-resources/src/resources/link_sys/tests/webhook_test.rs::test_default_config` (lines 5–18).

That test has ~10 assertions: five `assert!`, four `assert_eq!`, and one `matches!` guard.
Each targets a distinct contract: `auth` absent, method `"GET"`, timeout 5 000 ms, target variant `Uri`.
A single snapshot merges all of those into one opaque string, any unrelated new field forces a refill,
and the `matches!` structural invariant disappears entirely — a string snapshot cannot express it.

**Bad rewrite — do NOT do this:**

```rust
// BAD: single snapshot swallows all per-field contracts
let config = WebhookServiceConfig::default();
expect![[r#"
    WebhookServiceConfig {
        auth: None,
        request_method: "GET",
        timeout_ms: 5000,
        ...
    }
"#]].assert_debug_eq(&config);
```

**Keep it as individual `assert!` / `assert_eq!` per field.**

---

## §2 — How to write `expect-test`

### Dependency

Add to `Cargo.toml` under `[dev-dependencies]` (current pilot resolves `1.5.1`):

```toml
expect-test = "1.5"
```

### Import

```rust
use expect_test::expect;
```

### Template A — single-line string

Use for one-line output (decision-log header, status token).
`expect-test` writes the single-bracket form automatically when content fits on one line.
To create: write `expect![""]`, run refill, the library fills the string.

```rust
let log = PluginLog::render_for_test(&record);
expect!["ALLOW | rule=allow-all | src=127.0.0.1"].assert_eq(&log);
```

### Template B — multi-line structure

Use for YAML/JSON round-trips or `Debug` output.
`r#"..."#` preserves whitespace exactly; indentation matches actual output, not source indentation.

```rust
let yaml = serde_yaml::to_string(&config).unwrap();
expect![[r#"
    apiVersion: edgion.io/v1
    kind: EdgionPlugin
    metadata:
      name: cors
"#]].assert_eq(&yaml);
```

### Template C — large `Debug` dump (file-backed)

When an inline snapshot would dominate the test file (typically >30 lines), store it in a file.
The file is created on first refill and checked into source control alongside the test.

```rust
expect_file!["./snapshots/large_output.txt"].assert_eq(&big_debug_string);
```

### Refill workflow

This is the central operation. Run it, then **read the diff before doing anything else**.

```bash
# Select the workspace member that owns the snapshot.
# --lib         scopes to the library target (avoids running bin + integration targets)
# <filter>      module-path filter, e.g. plugins::http::dsl::tests::decision_log
UPDATE_EXPECT=1 cargo test -p edgion-gateway --lib <module::path::filter>

# MANDATORY: inspect every changed snapshot before moving on
git diff
```

> **Why `--lib`?** Without it, `cargo test` runs lib + bin + integration targets and emits multiple
> "Running unittests" sections. The relevant lib output can scroll past `tail` in a large build.
> Using `--lib` makes the output deterministic and prevents accidentally refilling unrelated targets.
> This is a real footgun encountered during the B1 pilot.

> **Note on `--test <suite>`.** That flag selects crate-level integration tests, such
> as the CRD guards under `edgion-resources/tests/`. Inline `#[cfg(test)]` modules use
> `-p <owning-crate> --lib <module-filter>` instead.

---

### Hard rule

> **Never run `UPDATE_EXPECT=1` without reading the diff.**
> A failing snapshot is a question the test is asking: "did this output change on purpose?"
> Auto-accepting collapses that question into silence and turns snapshots into a coverage-shaped no-op.
> If a PR lands a snapshot refill without a reviewer-visible justification in the diff, that is a
> process failure, not a tooling success.

Anti-pattern: a PR titled "fix flaky test" that contains only `UPDATE_EXPECT=1` output and no behavioral
explanation is almost always a silenced regression — treat it like a bare `#[allow(clippy::...)]`.

---

## §3 — Volatile fields (escape hatches)

### When to apply

The probe for volatile fields is **conditional**, not a ritual.
It applies only when the test input plausibly involves non-determinism: clocks, RNG, allocator addresses,
async ordering, or OS-dependent enumeration.

For literal-input / mocked-session / fixed-enum / in-process-buffer tests (like the B1 pilot in
`decision_log_tests.rs`), the probe is waived — output is deterministic by construction.
A one-sentence justification is sufficient: "input is a fixed `PluginLogRecord`; no clock or RNG involved."
Do not perform the probe ritually on every test.

### Volatility probe (when needed)

Run the suite ≥5 times with the raw output dumped to separate files, then diff them:

```bash
for i in $(seq 5); do
    cargo test -p edgion-gateway --lib <filter> -- --nocapture 2>&1 | grep -A50 'RUNNING' > /tmp/run_$i.txt
done
diff /tmp/run_1.txt /tmp/run_2.txt
```

Any divergence localizes the volatile substring.

### Fix order — follow (a) → (b) → (c); do not skip ahead

**(a) Inject determinism (preferred).**
Make the function under test accept a clock / RNG / ID generator parameter.
This removes volatility at the source and benefits all callers, not just tests.

**(b) Sanitize before comparing.**
Write a small local `sanitize()` helper that replaces volatile substrings with stable placeholders.
Prefer `str::replace` or `split` + `join` over pulling in a regex crate for trivial cases.

```rust
fn sanitize(s: &str) -> String {
    // Replace a dynamic request ID with a stable placeholder.
    s.split_once("req_id=")
        .map(|(prefix, rest)| {
            let after = rest.find(' ').map(|i| &rest[i..]).unwrap_or("");
            format!("{}req_id=<id>{}", prefix, after)
        })
        .unwrap_or_else(|| s.to_owned())
}

expect!["ALLOW | req_id=<id> | rule=allow-all"].assert_eq(&sanitize(&log));
```

**(c) Switch to `insta` with redaction.**
Use only when (a) is structurally blocked AND (b) would require a regex zoo larger than the test itself.

`insta` is not a project dependency as of 2026-05-27.
Introducing it requires:
1. Adding `insta = "1"` to `[dev-dependencies]`.
2. Documenting in the PR description why (a) and (b) were considered and rejected, with a worked example.
3. Optionally adding `cargo insta review` to a `cicd/` helper script (the repo has no `Makefile`).

Do not reach for `insta` before exhausting (a) and (b).

---

## §4 — Quantification rubric

After any snapshot conversion, report these four metrics.
Count only over the subset of tests **actually converted** — never the whole file.

| Metric | What to count |
|--------|---------------|
| Body lines, before / after | Lines between `#[test]` and the closing `}` of each converted `fn` |
| Assertion calls, before / after | `assert!` + `assert_eq!` + `expect!` invocations |
| Assertion strength delta | Qualitative: "strengthened", "equivalent", or "weakened" |
| Maintenance cost delta | Qualitative: "lower", "neutral", or "higher" |

**Body-line reduction is not guaranteed and is not the headline metric.**
The B1 pilot's body lines went **UP** +6% because of the comment block and retained negative-invariant
assertions in the partially-converted test #6.
What matters is assertion-strength and maintenance-cost, not line count.

A **"weakened"** result is a hard abort: revert the conversion, document it as a worked anti-example, and keep the originals.

A **"higher"** maintenance-cost result usually means a yellow-light case slipped through the §1 filter.

### B1 pilot measurements (`decision_log_tests.rs`, all 6 test bodies)

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Body lines | 67 | 71 | +4 / +6% |
| Assertion calls (`assert!` + `assert_eq!` + `expect!`) | 20 | 15 | −5 / −25% |
| `log.contains` probes | 14 | 3 (test #6 only) | −11 / −79% |
| Assertion strength (tests #1–#5) | Weak (substring probes) | Exact (full-string match) | Strengthened |
| Assertion strength (test #6) | Weak (4 probes) | Exact snapshot + 3 negative invariant probes | Strengthened, intent preserved |
| Maintenance cost | — | — | Lower |

---

## §5 — Registration

This skill is registered in `skills/05-testing/SKILL.md` Part 1 — Standard tests.

After modifying the registry, run:

```bash
python3 cicd/checks/validate_agent_docs.py
```

to confirm the registry is consistent with the files on disk.
