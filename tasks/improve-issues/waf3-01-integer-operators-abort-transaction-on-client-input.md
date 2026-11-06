# waf3-01: Numeric operators and setvar arithmetic abort the whole transaction on client-controlled non-i64 input; one CRS PL1 `Range` header turns every request into a 500

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/transaction.rs:5073-5087` (`CompiledOperatorOperand::Integer`: both the expanded operand and the input go through `parse_i64(...)?`)
  - `edgion-waf/src/seclang/transaction.rs:5353-5356` (`parse_i64`: strict `str::parse::<i64>()`, any failure is `RuntimeError::IntegerConversion`)
  - `edgion-waf/src/seclang/transaction.rs:2930-2954` (`setvar:+=`/`-=` with a macro operand: `parse_i64(&value)?`, overflow → `IntegerConversion`)
  - `edgion-waf/src/seclang/transaction.rs:2060`, `2111-2240` (the error propagates out of `evaluate_unit` and `execute_phase_with_prefix` — the phase is aborted, no rule is skipped)
  - `edgion-gateway/src/plugins/http/waf/mod.rs:178-199,308-319` (`IntegerConversion` falls into `_ => 500`; mode is ignored)
  - `config/waf/crs-4.25.1-supported-v1/upstream/rules/REQUEST-920-PROTOCOL-ENFORCEMENT.conf:310-330` (rule 920190, shipped in the supported core bundle)

## Summary
Edgion evaluates `@eq/@ge/@gt/@lt` and arithmetic `setvar` with a strict `i64` parse and treats any parse
failure (non-digits, empty, sign tricks, 20-digit numbers) as a fatal `RuntimeError`, not as a non-match.
The value being parsed is frequently client-controlled — directly (custom rules on `ARGS:x "@gt 10"`) or
through a capture. The bundled CRS 4.25.1 profile contains such a path at PL1: rule 920190 captures two
`\d+` groups from the `Range` header and compares them with `TX:2 "@lt %{tx.1}"`. A request carrying
`Range: bytes=99999999999999999999-1` makes `parse_i64("99999999999999999999")` fail, the request-headers
phase errors, and the Gateway returns 500. This happens in `DetectionOnly` too.

## Current behavior (evidence)
- 920190 (PL1, in `EdgionConfigData_crs-supported-core.yaml`, not in the 62 excluded units):
  `SecRule REQUEST_HEADERS:Range "@rx (\d+)-(\d+)" "...capture,...chain"` /
  `SecRule TX:2 "@lt %{tx.1}"`. Both captures are unbounded digit runs.
- `evaluate_operator` (transaction.rs:5073-5087): for a template operand it calls
  `parse_i64(&template_operand(...)?)?`, then `parse_i64(input)?`. `parse_i64` (5353-5356) returns
  `Err(IntegerConversion)` for anything `i64::from_str` rejects, including 19+-digit values above
  `i64::MAX`. The `?` returns the error from `inspect_predicate_stage_with_hit_matcher` →
  `evaluate_member_matches_into` → `evaluate_unit_into` → `execute_phase_with_prefix` (no per-rule
  containment).
- The same strictness applies to `setvar:'tx.x=+%{...}'` (2930-2954): the existing unit tests
  (`tests/unit/seclang/transaction_test.rs` around lines 1814-1920) deliberately assert that
  `?delta=invalid` makes `execute_request_headers()` return `Err(IntegerConversion)`, i.e. a client
  query string can fail the transaction.
- For JSON bodies the collector records the operator error as a "semantic error" outcome and replays it
  when the rule is reached (`json_inspection_collector.rs` `record_semantic_error`,
  `json_inspection_runtime.rs` `replay_static` → `OutcomeValue::Error(error) => return Err(error.error)`),
  so `{"page":"x"}` against `ARGS:page "@gt 10"` also fails the request.
- Gateway: `runtime_error_status` has no arm for `IntegerConversion` → 500, and `runtime_failure` does not
  consult `WafMode` (mod.rs:308-319). The supported-CRS README states "DetectionOnly suppresses rule
  interventions, not fatal runtime errors".

## Industry comparison
- ModSecurity v2/v3 numeric operators convert with `atoi`/`std::stoi`-style best-effort parsing; a
  non-numeric value becomes 0 and the rule simply evaluates. setvar arithmetic does the same.
- Coraza (`operators/gt.go`, `lt.go`, ...) uses `strconv.Atoi` and ignores the error (value 0 / clamped on
  range error); a bad value never aborts the transaction.
- CRS is authored against those semantics: rules such as 920190 assume "compare two captured numbers",
  never "a non-number is fatal".

## Impact
- Remote, unauthenticated, single-header availability break for every route protected by the shipped CRS
  profile (one malformed `Range` header → 500 for that request; trivially repeatable, cache-busting).
- `DetectionOnly` rollouts — the documented safe first step — are not safe: they 500 real traffic.
- Any operator-written numeric rule on request data (`ARGS`, headers, JSON fields) becomes a client-
  triggerable 500, and the failure is logged as a WAF engine error rather than as a rule hit.

## Suggested direction
1. Make numeric evaluation total: convert with ModSecurity-compatible lenient semantics (leading
   whitespace/sign, digits until the first non-digit, empty → 0, saturate to `i64::MIN/MAX` on overflow)
   for both operands and for `setvar` arithmetic. Keep strict parsing only for compile-time constants
   (already done in `semantic.rs` `strict_i64`).
2. If a strict mode is kept for engine testing, scope it to a non-fatal outcome (rule does not match +
   bounded diagnostic counter), never a transaction abort.
3. Add a regression fixture with `Range: bytes=99999999999999999999-1` and `?page=abc` against the
   supported profile in both `On` and `DetectionOnly`.

## Notes
- Distinct from `waf-05` (which covers body-parse/limit errors mapped to 500): here the request is
  well-formed and within limits; the fatal error is produced by ordinary rule evaluation semantics.
  Fixing `waf-05`'s status mapping alone would still fail these requests (as 4xx instead of 500).
- Checked `skills/04-review/waf/` (SKILL.md and all regression-guard pages): no page documents strict
  integer conversion as an accepted trade-off; the tests encode it but no rationale or CRS impact analysis
  exists.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `edgion-waf/src/seclang/transaction.rs:5073-5087`: an Integer operand from a template and the input both go through `parse_i64(...)?`. `parse_i64` (`:5353-5356`) returns `RuntimeError::IntegerConversion` for any value that `str::parse::<i64>` rejects, including 20-digit numbers.
- Rule 920190 (PL1, chain `TX:2 "@lt %{tx.1}"` over `(\d+)-(\d+)` captures) is in the shipped bundle: `upstream/rules/REQUEST-920-PROTOCOL-ENFORCEMENT.conf:310-330` and `generated/EdgionConfigData_crs-supported-core.yaml:7889`.
- Gateway: `runtime_error_status` (`edgion-gateway/src/plugins/http/waf/mod.rs:179-199`) has no arm for `IntegerConversion`, so it returns 500. `runtime_failure` (`:308-319`) ignores `WafMode`.
- No `skills/04-review/waf` page accepts strict conversion. It is distinct from waf-05, which already cross-references this root cause, so no merge.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `edgion-waf/src/seclang/transaction.rs:5353-5356` `parse_i64` is still strict `str::parse` → `IntegerConversion`; `evaluate_operator` (`:5073-5087`) and setvar arithmetic (`:2930-2954`) unchanged.
- Rule 920190 is still in the shipped supported core bundle (only `EdgionPlugins_crs-supported.yaml` quoting of `mode: "on"` and `acceptance.json` changed under `config/waf/`).
- Gateway still maps `IntegerConversion` to 500 (`waf/mod.rs:198`).
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
