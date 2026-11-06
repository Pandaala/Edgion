# waf-04: No rule-ID-scoped conditional exclusions (`ctl:ruleRemoveById` / `ruleRemoveTargetById`), forcing coarse tag-family exclusions for false positives

- **Category**: best-practice
- **Severity**: P3 — downgraded from P2 by independent verification
- **Scope**: local
- **Location**:
  - `edgion-waf/src/seclang/semantic.rs:4998-5129` (`parse_ctl_plan`; the compiler then admits only `forceRequestBodyVariable`, `requestBodyProcessor`, `ruleRemoveByTag`, `ruleRemoveTargetByTag`)
  - `edgion-waf/src/seclang/directive.rs:684-700` (`recognize`: only `SecRule`, `SecAction`, `SecMarker`, `SecComponentSignature`, `SecRuleUpdateTargetById`, `SecRuleRemoveById`)
  - `edgion-waf/src/seclang/tag_selection.rs:27-60` (static, configuration-only selection)
  - docs `docs/en/user-guide/http-route/filters/edgion-plugins/waf.md:374-394`

## Summary
False-positive tuning is the dominant operational task of any CRS deployment, and CRS documents one
canonical vocabulary for it: conditional runtime exclusions by rule ID (`ctl:ruleRemoveById`,
`ctl:ruleRemoveTargetById`) and static target updates by ID/tag/msg. Edgion supports only static
removal/target-update by ID (applies to every request) and conditional removal **by tag**. Because CRS rules
can neither be re-tagged (`SecRuleUpdateActionById` is unsupported and bundles are digest-pinned) nor
excluded by ID at runtime, an operator fixing one false positive on one parameter of one path must disable
an entire tag family (e.g. `attack-sqli`, ~60 rules) for that path, or remove the rule/target globally.

## Current behavior (evidence)
- Unknown directives are rejected (`recognize` → `None` → `DirectiveParseError::UnknownDirective`), so
  `SecRuleUpdateTargetByTag`, `SecRuleUpdateTargetByMsg`, `SecRuleRemoveByTag`, `SecRuleRemoveByMsg` and
  `SecRuleUpdateActionById` fail the bundle (strict, not silent — good).
- `parse_ctl_plan` rejects every other `ctl` command with `CtlErrorKind::Command`, including
  `ruleRemoveById` and `ruleRemoveTargetById`.
- `ruleRemoveTargetByTag=<tag>;<target>` is the finest conditional exclusion available; CRS tags are
  family-level (`attack-sqli`, `paranoia-level/2`, `OWASP_CRS`, `capec/...`).
- `WafRuleSelection` (include/exclude by tag/ID/phase) is static per profile; it cannot express "only for
  `REQUEST_FILENAME` `@beginsWith /admin/editor`".
- The CRS application exclusion plugins (WordPress, Drupal, Nextcloud, ...) are written with
  `ctl:ruleRemoveTargetById` and therefore cannot be loaded.

## Industry comparison
- ModSecurity v3 / Coraza: `ctl:ruleRemoveById`, `ctl:ruleRemoveByTag`, `ctl:ruleRemoveTargetById`,
  `ctl:ruleRemoveTargetByTag`, `SecRuleUpdateTargetBy{Id,Tag,Msg}`, `SecRuleRemoveBy{Id,Tag,Msg}` — the CRS
  "False Positives and Tuning" guide uses the ID-based runtime forms as its primary recipe.
- Cloudflare WAF/AWS WAF managed rules: per-rule "skip"/"exclude rule" with request-scoped expressions;
  Kong/Traefik Coraza integrations inherit the Coraza vocabulary.

## Impact
Operators trade large protection losses for small false-positive fixes, or keep the WAF in DetectionOnly.
This directly weakens the effective security of the "supported CRS" profile, and blocks migration of existing
ModSecurity/Coraza tuning files.

## Suggested direction
Add `ctl:ruleRemoveById=<id|range>` and `ctl:ruleRemoveTargetById=<id>;<target>` reusing the existing
request-scoped control-entry machinery already used for the tag variants (same budgets, same "later rules
only" semantics, same chain rollback). Optionally add `SecRuleUpdateTargetByTag` statically, which reuses the
`SecRuleUpdateTargetById` path. Then re-enable loading the official CRS exclusion plugins as bundles.

## Notes
Checked `skills/04-review/waf/regression-guards/request-scoped-control-actions.md` and
`static-tag-selection-before-compilation.md`: they record the currently supported set and its semantics, not a
decision that ID-scoped exclusions are out of scope. Not tracked under `tasks/` (the language-profile task is
about whole-family omission, which is the coarse approach this finding argues against).

## Verification (independent)

- **Verdict**: DOWNGRADED — P2 → P3. The claim is accurate: `semantic.rs` `parse_ctl_plan` admits `ruleRemoveByTag` (`:5072`) but no ID-scoped ctl. Two qualifications reduce the impact:
- The static rule selection supports rule IDs (`edgion-waf/src/seclang/tag_selection.rs:41-45`, `Node::RuleId`), and `SecRuleRemoveById`/`SecRuleUpdateTargetById` are supported. Because a WAF entry is per EdgionPlugins with its own entry `conditions`, a per-route (and condition-gated) ID exclusion is already expressible by splitting the policy.
- The real remaining gap is in-ruleset path-conditional ID exclusion and loading the CRS exclusion plugins verbatim. That is a compatibility and convenience improvement, not a forced loss of protection.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P3 unchanged.
- `edgion-waf/src/seclang/semantic.rs` `parse_ctl_plan` and `directive.rs` `recognize` are unchanged; no `ruleRemoveById`/`ruleRemoveTargetById` ctl exists.
- `tag_selection.rs:41-45` still supports static rule-ID selection, so the P3 downgrade rationale holds.
- `git diff feature-08-30..HEAD` shows no change under `edgion-gateway/src/plugins/http/waf/`, `edgion-gateway/src/ctx.rs`, `edgion-gateway/src/request_body/config.rs`, or `edgion-waf/src/` apart from a `serde_yaml`→`yaml_serde` dependency rename and a `fetch_update`→`try_update` rename in `json_inspection.rs:12`; none touch this finding's code paths.
