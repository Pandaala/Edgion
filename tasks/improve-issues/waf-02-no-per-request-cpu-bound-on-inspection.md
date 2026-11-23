# waf-02: No per-request CPU bound on WAF evaluation — 32 MiB default window, per-rule (not per-transaction) work budget, PikeVM-only regex, run inline on the Tokio worker

- **Category**: design-error
- **Severity**: P1
- **Scope**: large-direction
- **Location**:
  - `edgion-gateway/src/plugins/http/waf/mod.rs:518-538,700` (window defaults to the 32 MiB physical cap; synchronous `execute_request_body`)
  - `edgion-gateway/src/request_body/config.rs:6` (`DEFAULT_MAX_BODY_SIZE_BYTES = 32 MiB`)
  - `edgion-waf/src/seclang/transaction.rs:32-55` (runtime limits: value 4 MiB, operator work 64 MiB, transformation work 64 MiB)
  - `edgion-waf/src/seclang/transaction.rs:2112,2339,2407,2617,4747` (`unit_work_bytes` reset per rule unit; `charge_operator_work`)
  - `edgion-waf/Cargo.toml` (`regex-automata` with only `nfa-pikevm`), `edgion-waf/src/matcher.rs:899-1145` (`RegexCore` = PikeVM), `matcher.rs:1375-1395,1397-1420` (literal guard only for inputs ≥64 B with a ≥3-byte case-sensitive literal prefix)

## Summary
Every WAF budget is a per-rule byte budget; nothing bounds the total CPU a single request may consume.
With the default full window equal to the 32 MiB physical body cap, 4 MiB per value, a 64 MiB operator
work budget that is reset for every rule unit, and a PikeVM-only regex engine, one unauthenticated
request can drive hundreds of MiB to GiB of NFA simulation. All of it runs synchronously inside
`execute_request_body()` on the Pingora/Tokio worker thread, with no yield, deadline or offload.

## Current behavior (evidence)
- Full inspection window: `max_inspect_bytes.unwrap_or(physical).min(cap)` (mod.rs:525) — 32 MiB by default.
- `RuntimeLimits::default()`: `max_value_bytes` 4 MiB, `max_operator_work_bytes` 64 MiB,
  `max_transformation_work_bytes` 64 MiB, `max_operator_evaluations` 1,000,000 (a count, not bytes).
- `evaluate_unit` sets `self.unit_work_bytes = 0` for each unit (transaction.rs:2112/2339); the regression
  guard `chain-ordinary-work-accumulates-per-unit.md` confirms the budget is per rule unit. There is no
  transaction-wide aggregate.
- CRS 4.25.1 has ~155 `@rx` rules over `ARGS` in 93x/94x (`grep` of the pinned upstream rules); even at
  PL1 roughly half are active, each with 3-6 transformations charged as original+transformed bytes.
  A urlencoded body carrying one 4 MiB argument therefore costs on the order of 70 × (4 MiB × several
  stages) ≈ 1+ GiB of transformation + PikeVM work at PL1, more at PL4 — every rule stays below its own
  64 MiB budget, so no limit fires.
- Regexes compile only to `PikeVM` (Cargo features: `nfa-pikevm`; `RegexCore.vm: PikeVM`). PikeVM is the
  slowest regex-automata engine (it simulates all NFA states per byte); the optional literal guard is
  skipped for most CRS patterns because they start with `(?i)` classes or alternations.
- `run_request` calls `transaction.execute_request_body()` (mod.rs:700) with no `.await` inside; JSON/XML
  inspection likewise evaluates synchronously per chunk. No `spawn_blocking`, `yield_now`, or wall-clock
  check exists in `edgion-waf` or the WAF glue.

## Industry comparison
- ModSecurity separates `SecRequestBodyLimit` (13 MB, files) from `SecRequestBodyNoFilesLimit`
  (128 KB default in the recommended config) precisely because the non-file part is what ARGS rules scan.
  Coraza ships the same recommended defaults. Cloudflare/AWS WAF inspect only the first 8-64 KB of a body.
- ModSecurity/Coraza bound regex cost with PCRE match limits / RE2 linear DFA; Rust's own `regex` crate
  uses a bounded lazy DFA (`hybrid`) with automatic PikeVM fallback, giving linear time with a fixed cache
  and typically an order-of-magnitude higher throughput than PikeVM alone.
- Envoy's overload manager and Wasm filters bound per-request CPU via watchdog/deadlines rather than only
  per-operation byte counts.

## Impact
A handful of concurrent large-form requests can pin every gateway worker for seconds each, stalling
unrelated traffic on the same threads (a cheap application-layer DoS against the protection layer itself).
When a single rule does exceed 64 MiB the request fails with a 500 (see waf-05), so legitimate large forms
are also at risk. Gateway performance-baseline work is now explicitly untracked (`skills/04-review/waf/SKILL.md`
"Untracked validation and optimization"), and in any case would measure throughput rather than introduce a per-request bound.

## Suggested direction
1. Add a transaction-wide aggregate work budget (bytes and/or elapsed time checked between rule units)
   with a defined outcome (reject 413/503 or stop-and-flag, consistent with waf-01's ProcessPartial).
2. Default the inspected non-file body to a small window (ModSecurity's 128 KiB "NoFiles" model) instead of
   the 32 MiB physical cap; keep the physical cap for forwarding.
3. Enable regex-automata's `hybrid` lazy DFA with a fixed per-program cache capacity and PikeVM fallback
   (still linear-time, bounded memory — not the "unbounded regex DFA" excluded by `waf-dfa-state-layout.md`).
   The cache can live in the existing lazy per-transaction scratch slots.
4. For very large inspections, yield cooperatively between rule units (e.g. evaluate phase 2 in slices with
   `tokio::task::yield_now`) or move execution to a bounded blocking pool.

## Notes
Checked `skills/04-review/waf/regression-guards/chain-ordinary-work-accumulates-per-unit.md` (fixes chain
fan-out, explicitly keeps the per-unit bound), `native-injection-operators-use-bounded-detection.md`, and
the former DFA-state-layout task (phrase DFA only; now listed as untracked in `skills/04-review/waf/SKILL.md`). Related but distinct: [process-wide HTTP request admission](../todo/gateway-http-request-admission.md) and `gw-ext-03` (Wasm on Tokio workers). Throughput figures above are an order-of-magnitude
estimate from rule counts and limits; a release benchmark of one 4 MiB urlencoded argument against the
standard profile would quantify it.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 kept. The throughput figures are unmeasured estimates, so quantify them with the proposed 4 MiB-argument benchmark before sizing the fix.
- `edgion-waf/src/seclang/transaction.rs:2112,2339` reset `unit_work_bytes = 0` per rule unit, and `chain-ordinary-work-accumulates-per-unit.md` confirms the bound is per unit. No transaction-wide byte or time budget exists (no `Instant` or deadline in `transaction.rs`).
- `transaction.rs:32-55`: 4 MiB per value and 64 MiB operator/transformation work per unit. `DEFAULT_MAX_RUNTIME_INPUT_BYTES` is 16 MiB, which is somewhat lower than the 32 MiB window the finding cites but does not change the conclusion.
- `edgion-waf/Cargo.toml:11-14`: `regex-automata` has only the `nfa-pikevm` feature (no hybrid/lazy DFA).
- Execution is synchronous in the plugin (`waf/mod.rs` `execute_request_body`), with no yield or offload.

## Additional evidence (wave 3)
- The operator-facing JSON inspection budgets are not validated at all.
  `WafRequestBodyConfig::prefix_size_bytes` (`edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs:242-260`)
  is the only body-config check. It never looks at `requestBody.json.{lexical,structure,results}.*`
  (`waf.rs:99-131`, all plain `usize`). `WafJsonConfig::inspection_limits` (`waf.rs:143-171`) copies
  them verbatim into `JsonInspectionLimits`, replacing the engine defaults
  (`edgion-waf/src/seclang/json_inspection_runtime.rs:20-35`, for example `max_work: 128 MiB`). The
  Gateway uses them as they are (`edgion-gateway/src/plugins/http/waf/mod.rs:132-137,556-557`).
- So an `EdgionPlugins` author can raise `maxWork`, `maxEvents` or `maxRetainedBytes` to
  `usize::MAX`, which removes the remaining per-transaction JSON bound described here. Zero values
  are also accepted. The fix should give these fields ceilings equal to the engine defaults, enforced
  in the shared resource validation, so the Controller and Gateway agree.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `edgion-waf/src/seclang/transaction.rs:2112,2339` still reset `unit_work_bytes` per unit; `parse_i64`/limits at the same lines; `edgion-waf/Cargo.toml` still enables only `nfa-pikevm`.
- `skills/04-review/waf/SKILL.md` was rewritten: performance validation and the DFA/SIMD optimization tasks are now explicitly untracked. That adds no per-request CPU bound or accepted-design rationale; stale `tasks/todo/` references in this file were updated.
- `waf.rs` JSON budget fields remain unvalidated (resource file unchanged).
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
