# res3-03: Shared condition `regex` lists are joined with a bare `|`, so one pattern's inline flags leak into the others

- **Category**: design-error
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/conditions.rs:286-315` (`MatchCore::compile`)
  - `edgion-resources/src/resources/edgion_plugins/conditions.rs:697-706` (`validate_match_core`)
  - Compare `edgion-resources/src/resources/edgion_plugins/plugin_configs/cors.rs:143,156-166` (wraps each item)
  - Compare `edgion-resources/src/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs:149` (`\A(?:…)\z`)

## Summary
`MatchCore` is the shared engine for `keyMatch`, `jwtClaimMatch` and `mtlsMatch`. Those conditions
drive skip/run of every plugin, `RequestRestriction`, `RequestAccessPolicy`, dye rules and so on.
`MatchCore` compiles a list of patterns as `patterns.join("|")`. Each item is not wrapped in a
group, so the list is not the union of the individual patterns:
1. An inline flag at the start of one item (`(?i)`, `(?s)`, `(?x)`, `(?m)`) applies to the rest of
   the enclosing group. With no group, that is every following alternative. `["(?i)^/public/",
   "^/Admin$"]` makes `^/Admin$` case-insensitive.
2. Items that are invalid alone can combine into a valid but different regex. `["foo\\", "bar"]`
   becomes `foo\|bar`, a literal pipe. `["(a", "b)"]` becomes `(a|b)`. The Controller validates the
   same joined string, so admission accepts these lists.

## Current behavior (evidence)
```rust
// conditions.rs:296-301
if let Some(patterns) = &self.regex {
    if !patterns.is_empty() {
        let combined = patterns.join("|");
        self.compiled_regex = Some(regex::Regex::new(&combined)?);
    }
}
```
- The Controller runs the same `compile()` through `ConditionSet::compile`
  (`request_restriction.rs:178-180`, `request_access_policy.rs:427-429`, entry validation). It cannot
  detect either effect.
- In the Rust `regex` crate, `(?flags)` with no colon "sets flags for the rest of the current group".
  At top level, that covers later `|` alternatives.
- Other Edgion regex lists group each item. CORS wraps each user pattern as `^(p)$` and joins
  `(…)` groups (`cors.rs:143,156-166`). `hostFromHeader.allowRegex` is wrapped as `\A(?:p)\z`
  (`proxy_rewrite.rs:149`). `MatchCore` is the exception, and it backs the most security-relevant
  predicates.

## Industry comparison
- Envoy `StringMatcher`/`safe_regex`: each matcher compiles alone. A list means OR over separately
  compiled RE2 programs.
- `RegexSet` (Rust) and RE2 `Set` exist for this use case and keep patterns independent.

## Impact
- A list that mixes a case-insensitive item with case-sensitive items silently widens matching. In
  `skip` sets (for example, skip auth for public paths), more requests skip the plugin than the
  operator wrote. In `RequestRestriction` deny mode, the change can also narrow or widen what is
  blocked.
- A `negate: true` condition inverts the widened set.
- Invalid patterns can pass admission. The operator sees `Accepted=True` for an expression that is
  not the one they wrote.

## Suggested direction
- Compile `patterns.iter().map(|p| format!("(?:{p})")).collect::<Vec<_>>().join("|")`, or use
  `regex::RegexSet` / per-pattern `Regex` if per-item reporting is wanted. First compile each item
  alone so the error names the offending index. This matches CORS.
- If `caseSensitive: false` should apply to regex items, document it or apply `(?i)` per item.
  Today it applies only to `values` (`MatchCore::matches`, `conditions.rs:318-354`), which is another
  source of operator surprise.

## Notes
- `skills/04-review/deserialization/intentional-design/plugin-config-regex-dfa-size-limit.md`
  accepts the *size-limit* tradeoff for `MatchCore` and explicitly mentions "joins the
  operator-authored pattern list before compiling". It does not cover join semantics, and this
  finding does not ask for a size limit.
- `skills/04-review/tls/regression-guards/allowed-san-regex-case-mode.md` shows the project already
  treats inline `(?i)` scope as meaningful for SAN regexes.

## Verification (independent)

Verdict: CONFIRMED, DOWNGRADED P2 -> P3.
- `MatchCore::compile` joins with a bare `|` (`conditions.rs:296-301`); `validate_match_core` (`:697-706`) checks only presence, so the joined form is the only thing validated.
- CORS (`cors.rs:156-179`) wraps items; MatchCore is the outlier. Rust `regex` top-level `(?flags)` applies to the rest of the group, i.e. later alternatives.
- Downgrade reason: the effect requires an operator to place a leading inline flag or a deliberately unbalanced fragment in one list item; ordinary anchored/plain patterns behave as a union. Cheap, correct fix, but not a likely-to-trigger security gap.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `edgion_plugins/conditions.rs` is unchanged; `MatchCore::compile` still uses a bare `patterns.join("|")`.
