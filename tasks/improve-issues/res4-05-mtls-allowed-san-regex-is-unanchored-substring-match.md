# res4-05: mTLS `allowedSans` RegularExpression entries are evaluated as unanchored substring searches

- **Category**: design-error
- **Severity**: P2 (security)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/allowed_san.rs:107-131` (`AllowedSan::compile` / `compile_bounded_regex`: pattern compiled verbatim)
  - `edgion-common/src/allowed_san.rs:105-118` (`match_presented_value`: `regex.is_match(presented)`)
  - `docs/en/ops-guide/gateway/tls/edgion-tls.md:121`, `skills/02-features/03-resources/09-edgion-tls.md:97-101` (no anchoring semantics documented)

## Summary

`EdgionTls.spec.clientAuth.allowedSans[]` is an authorization allowlist for client certificates. For
`match: RegularExpression` the operator's pattern is compiled as-is and evaluated with `Regex::is_match`,
which in the Rust `regex` crate is a *search* (succeeds if the pattern matches anywhere in the SAN). An
entry written the way operators write it for every other proxy -
`value: "spiffe://cluster.local/ns/prod/sa/api"` or `value: "api\\.internal\\.example\\.com"` - also
admits `spiffe://cluster.local/ns/prod/sa/api-debug`, `.../ns/prod-sandbox/...`-style variants or
`evil-api.internal.example.com.attacker.test`, as long as the certificate chains to the configured CA.

## Current behavior (evidence)

- `compile_bounded_regex(pattern, ignore_case)` (`allowed_san.rs:117-131`) builds `RegexBuilder::new(pattern)`
  with no implicit `^(?:...)$` wrapping.
- `match_presented_value` (`edgion-common/src/allowed_san.rs:114-117`) returns
  `regex.is_match(presented)`; there is no `find`/length check.
- Exact/Prefix/Suffix/Contains are separate, explicit match kinds, so an operator selecting
  `RegularExpression` has no indication that it behaves like an implicit `Contains` with regex syntax.
- `allowed_san_config_error` only checks compilation bounds; it does not warn about unanchored patterns.

## Industry comparison

- Envoy `StringMatcher.safe_regex` (used by `match_typed_subject_alt_names`) is a full match: "The regex
  match string. The string must match the regex" (RE2 `FullMatch`).
- Istio `AuthorizationPolicy` principals and Gateway API `RegularExpression` matchers in conformant
  implementations are effectively full-string matches.

## Impact

Workloads issued certificates by a shared CA (SPIFFE/mesh CA, enterprise PKI issuing many client certs)
can satisfy another workload's SAN allowlist whenever their SAN contains the allowed pattern as a
substring. Because mTLS SAN allowlists are commonly the only per-client authorization layer for
partner/internal APIs, a silent widening of the allowlist is an authorization bypass.

## Suggested direction

- Evaluate RegularExpression entries as full matches: compile `^(?:<pattern>)$` (keeping the existing size
  bounds) in `compile_bounded_regex`, so Controller validation and Gateway runtime stay identical.
- Pre-release, no compatibility shim is needed; document the full-match semantics in the EdgionTls feature
  schema and operator guide.
- Audit other operator-facing `RegularExpression` matchers that use `is_match` on security decisions
  (e.g. `KeyMatchMode::Regex` in `edgion-resources/src/matcher/key_group_helpers.rs`) for the same
  convention, and make the choice explicit in `skills/00-design-rules.md`.

## Notes

- Checked `skills/04-review/tls/regression-guards/allowed-san-regex-case-mode.md` (case-folding only) and
  `typed-san-display-prefix-not-dns.md`; neither records a decision on anchoring. Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (P2)
- `edgion-resources/src/resources/allowed_san.rs:117-131` `compile_bounded_regex` passes the operator pattern verbatim to `RegexBuilder::new` with no `^(?:...)$` wrapping.
- `edgion-common/src/allowed_san.rs:114-117` evaluates `regex.is_match(presented)`, a substring search in the Rust `regex` crate.
- `skills/04-review/tls/regression-guards/allowed-san-regex-case-mode.md` covers case folding only; neither the feature schema nor the ops guide documents anchoring semantics, while Envoy `safe_regex` SAN matching is a full match.
- Not a duplicate of `res3-03` (Condition regex list joining).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `edgion-resources/src/resources/allowed_san.rs` and `edgion-common/src/allowed_san.rs` are unchanged; `match_presented_value` still calls unanchored `regex.is_match`.
- `docs/en/ops-guide/gateway/tls/edgion-tls.md` changed only TLS-default inheritance text (still no anchoring semantics); the `allowedSans` row has shifted to around line 128.
