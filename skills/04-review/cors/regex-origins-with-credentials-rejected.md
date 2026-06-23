---
name: regex-origins-with-credentials-rejected
description: Use when reviewing CORS allow_credentials wildcard-guard findings; documents that allow_origins_by_regex / timing_allow_origins_by_regex are now rejected when allow_credentials is true (fixed fr-auth2-01), and why a per-pattern width audit was not accepted.
---

# CORS Regex Allowlists Rejected With `allow_credentials` (Fixed)

## Conclusion

The `allow_credentials: true` validation guard in `CorsConfig` now rejects **non-empty
`allow_origins_by_regex` and `timing_allow_origins_by_regex`** in addition to the literal
`*` / `**` checks on `allow_origins` / `allow_methods` / `allow_headers` /
`expose_headers` / `timing_allow_origins`. Fixed 2026-05-29 (finding `fr-auth2-01`, P2
Security).

Operators who need both regex origin matching and credentials must instead use an
explicit comma-separated `allow_origins` list.

## The defect (pre-fix)

Before the fix the guard only called `.contains('*')` on the literal string fields. A
catch-all regex (`allow_origins_by_regex: [".*"]`, `"^https?://.*"`, etc.) combined with
`allow_credentials: true` passed validation silently, because the regex list was only
checked for compile-validity, never for semantic width. At runtime:

1. `is_origin_allowed("https://attacker.com")` matches the `.*` regex → `true`
2. `select_acao_value` echoes the request origin back (because `allow_origins != "*"`)
3. `build_cors_header_pairs` emits `Access-Control-Allow-Origin: <attacker-origin>` together
   with `Access-Control-Allow-Credentials: true`

That is the browser-honored prohibited combination the guard exists to prevent — the same
combination that the literal `*`+credentials case is rejected for.

## Reasons

1. **Regex width cannot be reliably statically checked**: `.*`, `^https?://.*`,
   `^https://.*` and many other plausible operator shortcuts all match any origin; there
   is no robust way to classify an arbitrary user regex as "narrow enough" to safely pair
   with credentials.
2. **The literal-`*` guard already establishes the policy**: rejecting wildcard-equivalent
   origins under credentials is existing, intentional CORS-spec behavior. The regex escape
   path was simply an unguarded hole in the same policy, not a separate design decision.
3. **Explicit allowlist is the safe escape hatch**: operators who genuinely need credentials
   can enumerate origins in `allow_origins`; this keeps the credentialed surface auditable.

## Fix suggestions NOT accepted

- **Per-pattern width audit / "allow narrow regex, reject only `.*`"** — there is no reliable
  heuristic for "narrow"; a deny-all rule is the only safe minimal fix.
- **A new opt-in config knob (e.g. `allowRegexWithCredentials: bool`)** — adds a footgun that
  re-opens exactly this hole; not accepted.
- **Runtime-side mitigation (refuse to echo on regex match when credentials enabled)** — the
  validation layer is the correct place; the data-plane path must not grow conditional
  observability/branching for this (Iron rules on hot path).

## Decision Rule

When a review/audit report contains patterns of the form:

1. "`allow_credentials` guard checks `allow_origins.contains('*')` but not
   `allow_origins_by_regex` / `timing_allow_origins_by_regex`"
2. "`allowOriginsRegex: [\".*\"]` + `allowCredentials: true` is accepted → any origin gets
   credentialed CORS"

verify the fix is present before closing as re-report:

- The `allow_credentials` block in both `CorsConfig::validate_and_init` and
  `PluginValidation::validation_error` (`cors.rs`) rejects non-empty
  `allow_origins_by_regex` and `timing_allow_origins_by_regex`.
- `test_regex_origins_with_credentials_is_error` and
  `test_timing_regex_origins_with_credentials_is_error` still exist and assert both
  validation entry points error.

Both satisfied → close as already-fixed (`fr-auth2-01`).

## Re-evaluation Triggers

- If a reliable static "regex width" classifier is ever introduced, the deny-all rule could
  be relaxed to allow audited narrow patterns.
- If operator feedback shows the explicit-list escape hatch is unworkable for a real use
  case, reconsider an opt-in knob (with a loud documented warning).

## Background Notes

- Decision date: 2026-05-29
- Finding source: `fr-auth2-01` (Opus-verified CONFIRMED, P2 Security)
- Related: `subdomain-wildcard-https-only.md` (the `*.domain` shorthand goes through
  `allow_origins` and IS caught by the `contains('*')` guard — only the regex path was the
  hole); `get-validated-origin-split.md` is unrelated (a `select_acao_value` refactor).
