# waf4-02: `ruleSelection.include` has no dependency closure, so a tag/phase/ID include silently removes CRS initialization and blocking-evaluation units and the WAF stops blocking while Accepted

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/tag_selection.rs:865` (`selected = unit.is_active() && include && !exclude`, applied uniformly to every rule unit)
  - `edgion-waf/src/seclang/tag_selection.rs:770-800` (tag atoms match only literal `tag:` actions on the unit's members)
  - `edgion-waf/src/seclang/include.rs:1229-1300` (selection mask applied before the second compilation; only invalid-rule and body-admission checks follow)
  - `edgion-waf/src/seclang/semantic.rs:1991-2031` (`apply_selection_mask` deactivates units; no check on what the remaining units depend on)
  - CRS 4.25.1 data: `REQUEST-901-INITIALIZATION.conf` (901001/901100/... carry only `tag:'OWASP_CRS'`), `REQUEST-949-BLOCKING-EVALUATION.conf:226-232` (949110 carries `anomaly-evaluation`, `OWASP_CRS`), and the per-file `TX:DETECTION_PARANOIA_LEVEL "@lt N" ... skipAfter` gates (`tag:'OWASP_CRS'` only)

## Summary
Static rule selection evaluates `include && !exclude` for every complete rule unit and has no idea which
units are infrastructure. CRS is not a set of independent detectors. Initialization (901*) sets the
paranoia levels and anomaly thresholds. Paranoia gate rules (9xx011..9xx018) skip higher-PL rules.
Blocking evaluation (949110/959100/980*) turns accumulated scores into a deny. None of these carry an
attack tag. An intuitive profile such as `include: {tag: attack-sqli}` or
`include: {anyOf: [{tag: attack-sqli}, {tag: attack-xss}]}` keeps only the scoring rules. It drops
everything that makes them block. The result compiles, publishes as Accepted with only a numeric
summary, and runs as a detect-nothing/block-nothing WAF. Every PL1-PL4 rule now runs ungated, so false
positives in logs rise while nothing is denied.

## Current behavior (evidence)
- `PreparedWafRuleSelection::select` (tag_selection.rs:838-880) computes `selected` per unit and only reports
  `selected_units`, `excluded_units`, `unmatched_atoms` and `empty_selection`. There is no notion of
  "required" units, and markers are the only structural elements retained.
- 949110 matches neither `{tag: attack-sqli}` nor `{phase: 1}`, so it is excluded. 901* initialization is
  excluded the same way. With 901* gone, `tx.inbound_anomaly_score_threshold` is never set. Even when an
  operator keeps 949110 (for example `include: {anyOf: [{tag: attack-sqli}, {tag: anomaly-evaluation}]}`),
  `@ge %{tx.inbound_anomaly_score_threshold}` (949.conf:212/225) expands to an empty operand. Per waf3-01,
  that aborts the transaction instead of blocking.
- The skill page `static-tag-selection-before-compilation.md` states that "Unmatched atoms and valid empty
  selections do not add resolution warnings or imply PartiallyInvalid". A selection that keeps hundreds of
  detection units but zero blocking units is not even "empty", so nothing is surfaced.
- All shipped examples (`config/waf/crs-4.25.1-supported-v1/language-selection-examples/*.yaml`) and the
  user guide use only `exclude`. The resource corpus `edgion-resources/tests/waf_tag_selection.rs` has no
  `include` case. So the dangerous half of the feature has no audited usage.

## Industry comparison
- ModSecurity/Coraza expose only *removal* (`SecRuleRemoveByTag/ById`, `ctl:ruleRemoveByTag/Id`). CRS
  documentation tells operators to tune by exclusion and never to remove 901/949/959/980.
- CRS 4 handles optional rule families with *plugins* and `tx.*` toggles (`tx.crs_skip_...`,
  `tx.enable_*`), which disable detections while the setup and blocking machinery stay intact.
- Cloudflare/AWS WAF managed rulesets that allow per-tag selection keep scoring and threshold logic out of
  the selectable set entirely.

## Impact
An operator who reads "include = rules matching this condition" (the natural meaning) silently disables
enforcement for every route bound to that profile. The status stays `Accepted=True`. The only signal is
a count in the Accepted message or in `edgion-cli waf tags --excluded`. Requests that should be denied
by anomaly scoring are forwarded.

## Suggested direction
Options, in order of preference:
1. **Protect infrastructure units.** Compute the set of units that selection may never deactivate: units
   with no metadata leaves matched by any detection tag, units that only `setvar` TX configuration or
   `skipAfter` (initialization and PL gates), and any unit whose disruptive action reads anomaly totals.
   Make `include` apply only to detection units. Alternatively, require the bundle to declare a
   `required` tag (the guide's example already hints at `always-required`) that selection cannot remove.
2. **Fail validation** (Accepted=False/PartiallyInvalid) when the selected set keeps scoring units but
   removes every unit with a disruptive action in the blocking phase, or removes units that define TX
   variables still referenced by macros in selected units. The compiler already rebuilds target
   reachability in `apply_selection_mask`, so the same pass can check macro/`TX` definitions.
3. At minimum, drop `include` (keep exclude-only, matching ModSecurity/CRS practice), or emit a Warning
   condition naming the removed blocking/initialization unit IDs.

Recommendation: (1) plus the TX-definition check from (2). Exclusion-only usage keeps working, and
`include` gains the meaning operators expect.

## Notes
- Checked `skills/04-review/waf/regression-guards/static-tag-selection-before-compilation.md` and the user
  guide (`docs/en/user-guide/http-route/filters/edgion-plugins/waf.md` "Static rule selection"). They
  document exact-literal matching and that empty/unmatched selections may publish. They do not discuss
  CRS unit dependencies. The new evidence is that the CRS 4.25.1 setup, PL gate and blocking units carry
  no attack tag, so every include-by-attack-tag selection is a silent fail-open. That is not an
  "empty selection".
- Related: waf-04 (no rule-ID runtime exclusions), waf3-01 (empty numeric operand aborts the transaction).

## Verification (independent)

- **Verdict**: CONFIRMED (P2)
- `edgion-waf/src/seclang/tag_selection.rs:865` computes `selected = unit.is_active() && include && !exclude` uniformly for every unit; the summary (`:866-880`) reports only counts and `empty_selection`, with no notion of required units.
- `config/waf/crs-4.25.1-supported-v1/upstream/rules/REQUEST-949-BLOCKING-EVALUATION.conf:226-233`: 949110 (`deny`) carries only `anomaly-evaluation` and `OWASP_CRS` tags; the PL gates (949011/949012, `:235-236`) and REQUEST-901 rules carry only `OWASP_CRS`, so `include: {tag: attack-sqli}` excludes all of them.
- `skills/04-review/waf/regression-guards/static-tag-selection-before-compilation.md:41,118-119` documents literal `include && !exclude` and that valid selections add no warnings; it does not consider CRS unit dependencies, so this is new evidence rather than a re-raise of accepted design.
- Distinct from `waf-04` (runtime rule-ID exclusions) and `waf3-01` (empty operand abort), which it references.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P2 unchanged.
- `edgion-waf/src/seclang/tag_selection.rs:865` still computes `selected = unit.is_active() && include && !exclude` with no required-unit notion; `include.rs`/`semantic.rs` `apply_selection_mask` unchanged.
- `static-tag-selection-before-compilation.md` and the CRS 949/901 data are unchanged.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
