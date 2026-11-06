# waf-05: Client-caused body parse errors and input-limit exhaustion return 500 in `On` mode, with no rule-visible error variable

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/http/waf/mod.rs:179-199` (`runtime_error_status`: only byte-limit errors map to 413, everything else `_ => 500`)
  - `edgion-gateway/src/plugins/http/waf/mod.rs:308-319` (`runtime_failure` ignores `WafMode`)
  - `edgion-waf/src/seclang/transaction.rs:1540-1562` (malformed body → `RuntimeError::Body` in `On`; fallback only in `DetectionOnly`)
  - `edgion-waf/src/seclang/body.rs:583-680` (multipart: any deviation → `BodyError::Malformed`)
  - `edgion-waf/src/seclang/transaction.rs:32-55` (input limits: 1024 headers, 4096 query args/cookies, 4 MiB value, per-unit work 64 MiB)

## Summary
Request-body parse failures (invalid JSON/XML under a JSON/XML content type, multipart with LF line
endings, a part without `name`, an unquoted boundary edge case) and exhaustion of client-driven input
limits (too many headers/args/cookies, oversized single value, operator-work limit) all terminate the request
with **500 Internal Server Error** when the WAF is `On`. The failure is decided by the engine, not by rules:
there is no `REQBODY_ERROR`/`MULTIPART_STRICT_ERROR` equivalent that a rule (or an operator exclusion) could
inspect, score or exempt.

## Current behavior (evidence)
- `publish_non_json_body` falls back to a raw publication for `Malformed`/`MultipartBoundary` only when
  `rule_engine_mode == DetectionOnly`; in `On` it returns `RuntimeError::Body { kind }` (transaction.rs:1540-1561).
  JSON/XML lexical errors follow the same pattern through `JsonChecker`/`XmlStream` errors.
- `runtime_error_status` maps only `BodyError::ByteLimit` and JSON/XML `SourceBytes` to 413; every other
  error — `Malformed`, `QueryArgumentLimit`, `HeaderLimit`, `ValueByteLimit`, `OperatorWorkLimit`,
  `CollectionEntryLimit` — hits `_ => 500` (mod.rs:198).
- `runtime_failure` returns `ErrResponse { status, body: None }` regardless of the plugin mode (mod.rs:308-319);
  the access log carries a `WafErrorCode` such as `BodyProcessing`/`InputLimit`.
- `RuntimeError` variants are not surfaced as SecLang variables; `CollectionKind` has no
  `REQBODY_ERROR`/`REQBODY_PROCESSOR_ERROR`/`MULTIPART_*` error collections (semantic.rs:171-181).

## Industry comparison
- ModSecurity/Coraza set `REQBODY_ERROR`, `REQBODY_ERROR_MSG`, `MULTIPART_STRICT_ERROR` and continue; the
  recommended config's rules 200002/200003 deny with **400**, and operators can exempt a path with a
  preceding `ctl` rule. Unparseable input is a client error, not a server error.
- Envoy/Kong/APISIX: local-reply 400 for malformed client input, 413 for size, 431 for header limits;
  5xx is reserved for proxy/upstream faults.

## Impact
- Clients can trivially generate gateway-attributed 5xx (one malformed JSON POST), inflating error-rate SLOs,
  triggering 5xx-based alerting/outlier logic, and hiding real gateway faults.
- Legitimate traffic that a backend would accept (LF-only multipart from some clients, JSON with a
  non-standard but tolerated content) is hard-failed with no tuning path short of DetectionOnly or removing
  the WAF from the route.

## Suggested direction
1. Split runtime errors into client-input errors (parse/malformed, per-request input-count/size limits) and
   internal errors (allocation, arithmetic, invariants, generation state). Map the former to 400/413/431 and
   keep 500 only for the latter.
2. Expose a rule-visible body-error variable (ModSecurity-compatible `REQBODY_ERROR`/`REQBODY_ERROR_MSG`) and
   ship the equivalent of rule 200002 in the supported CRS profile, so the decision is a rule and can be
   excluded per path via ctl (see waf-04). Keep the current engine-level fail-close for genuine internal faults.

## Notes
Docs (`waf.md:445-449`) and `skills/04-review/waf/regression-guards/native-xml-inspection.md` state that
DetectionOnly suppresses rule interventions but not fatal errors, and that malformed DetectionOnly input falls
back. Neither records a decision about the HTTP status class or about exposing body errors to rules; the
500 mapping is a catch-all arm, not a reviewed choice. Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P2 unchanged.
- `waf/mod.rs:179-199` `runtime_error_status` maps only `BodyError::ByteLimit` and JSON/XML `SourceBytes` to 413. Everything else (malformed body, header/arg/value limits, operator work limit) falls to `_ => 500`.
- The docs (`waf.md:684`, "DetectionOnly does not suppress fatal runtime failures") describe fail-close but not the status class. No review page decides that client-input errors should be 5xx.

## Additional evidence (wave 3)
- Rule *evaluation* (not only body parsing/limits) can raise client-triggered fatal errors that land in the
  same `_ => 500` arm: `RuntimeError::IntegerConversion` from strict `parse_i64` in numeric operators and
  `setvar` arithmetic (`edgion-waf/src/seclang/transaction.rs:5073-5087,5353-5356,2930-2954`), and
  `CaptureByteLimit` when a `capture` rule matches a value whose captured groups exceed 256 KiB
  (`transaction.rs:3561-3577`, default `DEFAULT_MAX_RUNTIME_CAPTURE_BYTES`). For JSON/XML bodies these are
  recorded as provisional "semantic errors" and re-raised when the rule is reached
  (`json_inspection_runtime.rs` `replay_static`). The shipped CRS PL1 rule 920190 reaches the integer path
  with a single `Range` header; see `waf3-01` for the root cause and fix. Any status-mapping fix here should
  treat these as client errors too, but the semantic fix belongs in `waf3-01`.

## Additional evidence (wave 3)
- JSON body inspection has its own fixed, client-reachable result limits:
  `ResultLimits::default()` (`edgion-waf/src/seclang/json_inspection.rs:30-41`: 65,536 candidates,
  1,000,000 records, 32 MiB retained, 128 Mi work units, 4,096 deferred entries) and
  `JsonStructureLimits::default()` (`json_checker.rs:27-35`: 65,536 object members, 16 KiB path, depth 64).
  With CRS loaded, every scalar is retained as a candidate (`json_inspection_collector.rs:275-306`,
  `needs_all_candidate_names`). A ~131 KB body such as `[0,0,0,...]` with 65,537 elements therefore exceeds
  the candidate limit. `map_store_error` (`json_inspection_runtime.rs:71-92`) turns that into
  `RuntimeError::JsonResultLimit`, and `runtime_error_status`
  (`edgion-gateway/src/plugins/http/waf/mod.rs:179-199`) maps it to 500 (error code `RuntimeLimit`).
  So the 500 is reachable with a small, syntactically valid JSON document, far below any body-size limit.
  The per-candidate profile lists are walked linearly (`find_profile`, `json_inspection.rs:933-951`) and
  every step charges the same 128 Mi work budget. That gives a second, shape-dependent way to reach the
  same 500.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-gateway/src/plugins/http/waf/mod.rs:179-199` `runtime_error_status` still ends in `_ => 500` (`:198`); `runtime_failure` (`:308-319`) still ignores `WafMode`.
- `transaction.rs:1540-1562` still falls back only in `DetectionOnly`; no `REQBODY_ERROR`-style collection was added.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
