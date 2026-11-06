# waf-01: Body inspection cutoff (default `onLimit: continue`) skips every remaining WAF phase, including non-body rules and CRS blocking evaluation

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/http/waf/mod.rs:144-170` (`inspection_stop`)
  - `edgion-gateway/src/plugins/http/waf/mod.rs:518-538` (effective window = `min(maxInspectSize | physical, storage capture limit)`)
  - `edgion-gateway/src/plugins/http/waf/mod.rs:589,618,637` (the three cutoff exits, all before `execute_request_body` at :700)
  - `edgion-gateway/src/ctx.rs:3181-3200` (`stop_waf_inspection`: `claimed_phases = u8::MAX`, `transaction = None`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/waf.rs:174-180` (`WafInspectionLimitAction::Continue` is the default)
  - `edgion-gateway/src/request_body/config.rs:26-33` (`capture_limit` = 128 KiB memory buffer when `spillToDisk: false`)

## Summary
When a request body is larger than the effective full-inspection window, the WAF does not perform a
partial inspection: it drops the transaction, marks all phases claimed and forwards the request
(default `onLimit: continue`). Phase 2 therefore never runs — not only body rules, but also the
query-string/URI/header/cookie rules that CRS places in phase 2 and the CRS anomaly blocking
evaluation (949110). Any attacker can bypass the entire WAF by padding the body one byte past the
window. With memory-only body storage the window silently becomes 128 KiB.

## Current behavior (evidence)
- `run_request` computes `window = maxInspectSize.unwrap_or(physical).min(cap)` and, when the WAF is the
  sole body consumer, `effective = window.min(request_body_config.capture_limit())` (mod.rs:525-532).
- If the captured body exceeds `effective` (JSON/XML `InspectionOutcome::Limited` :589, native prefix
  capture incomplete :618, complete capture longer than window :637), `inspection_stop` is returned.
- `inspection_stop` → `ctx.stop_waf_inspection` sets `claimed_phases = u8::MAX` and drops the
  transaction (ctx.rs:3191-3192); with `Continue` it returns `GoodNext` (mod.rs:167-169). Phase 2
  (`execute_request_body`, mod.rs:700) and phases 3-5 never execute.
- In CRS anomaly mode almost all attack rules (930-944) are `phase:2` and target `ARGS` (which includes
  `ARGS_GET`), `REQUEST_COOKIES`, `REQUEST_HEADERS`, `REQUEST_FILENAME`; the only blocking decision is
  949110 in phase 2 (the shipped profile sets `early_blocking=0`, `config/waf/crs-4.25.1-supported-v1/README.md`).
  So phase-1 detections are logged but cannot block either ("Phase 1 findings remain" — docs waf.md:435).
- `capture_limit()` is `min(max_buffered, max_body)` with spill, but only `memory_limit` (default 128 KiB)
  when `spillToDisk: false` — a common setting on read-only container filesystems.

Example: `GET`-style SQLi in the query string plus a 130 KiB junk form body (memory-only storage), or a
body of `maxInspectSize + 1` bytes: zero WAF rules evaluate the query string.

## Industry comparison
- ModSecurity/Coraza `SecRequestBodyLimitAction ProcessPartial` inspects the first N bytes and still runs
  every rule; the recommended config (`modsecurity.conf-recommended`, Coraza `coraza.conf-recommended`)
  defaults to `Reject`.
- AWS WAF "oversize handling" (`CONTINUE | MATCH | NO_MATCH`) applies only to the body component; header,
  URI and query-string statements are still evaluated. Cloudflare inspects the first N KB of the body and
  evaluates the rest of the ruleset normally.

## Impact
Trivial, deterministic full WAF bypass for anyone who can send a body, whenever an operator sets
`maxInspectSize` (the documented knob for bounding inspection cost) or disables disk spill. The bypass is
recorded only as `outcome=Incomplete` in the access log.

## Suggested direction
1. Change the default `onLimit` to `reject` (413), matching the ModSecurity/Coraza recommended posture.
2. Redefine `continue` as true ProcessPartial semantics: publish the inspected window as a truncated body
   (raw `REQUEST_BODY`/urlencoded/multipart parts that are complete within the window; structured
   JSON/XML can fall back to raw-only) and still run phases 2-5 so non-body rules and 949110 evaluate.
   At minimum, when the body cannot be published, still execute phase 2 with an empty/unavailable body
   view instead of skipping it.
3. Expose an explicit variable (e.g. `INBOUND_DATA_ERROR`/`REQBODY_ERROR`-like) so rules can score
   truncated inspection, as CRS 920 does for ModSecurity.

## Notes
Checked `skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md` ("A policy cutoff ...
stops all remaining WAF phases") and docs `waf.md:430-435`. The behavior is documented, but no page
analyzes that it also disables non-body phase-2 rules and the CRS blocking evaluation, nor the 128 KiB
memory-only threshold; this is new evidence that the documented semantics are a security hole, not an
accepted tradeoff. Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged. The precondition is that `maxInspectSize` is set, or that memory-only storage is used while the WAF is the sole body consumer. With defaults (window = physical cap, spill enabled) the cutoff does not trigger before the physical 413.
- `waf/mod.rs:144-170` `inspection_stop` calls `stop_inspection` and returns `GoodNext` unless `onLimit == Reject`. `waf.rs:177-178`: `Continue` is the `#[default]`.
- `waf/mod.rs:515-536`: `window = maxInspectSize.unwrap_or(physical).min(cap)` and `effective = window.min(capture_limit)` when partial capture is allowed.
- `http-body/regression-guards/waf-prefix-before-dispatch.md` documents that a cutoff "stops all remaining WAF phases" and that "Phase 1 findings remain". It records the behavior but does not analyze that CRS phase-2 ARGS_GET/header rules and 949110 blocking are skipped too. That is the new evidence, so this is not a closed accepted tradeoff.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P1 unchanged.
- `edgion-gateway/src/plugins/http/waf/mod.rs:144-170` (`inspection_stop` → `GoodNext` unless `Reject`) and `ctx.rs:3191-3192` (`claimed_phases = u8::MAX`, `transaction = None`) are unchanged.
- `edgion-resources/.../plugin_configs/waf.rs` is unchanged; `Continue` remains the default `onLimit`.
- The only edit to `skills/04-review/http-body/regression-guards/waf-prefix-before-dispatch.md` is a Pingora branch-name update; it still adds no analysis of skipped phase-2 non-body rules / 949110.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
