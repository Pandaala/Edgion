---
name: get-validated-origin-split
description: Use when reviewing CORS plugin findings that propose splitting `get_validated_origin`, adding a `Cors::is_origin_allowed` wrapper, or adding a `has_credentials` parameter to ACAO value selection; documents the accepted split shape and rejects the two over-scoped variants.
knowledge_type: regression-guards
---

# CORS `get_validated_origin` Split — Value Selection Extracted (Fixed)

## Conclusion

`Cors::get_validated_origin` (in `edgion-gateway/src/plugins/http/cors/plugin.rs`)
keeps the allow/deny decision and now delegates the
Access-Control-Allow-Origin value choice to a separate, independently
unit-tested helper:

```rust
fn select_acao_value(&self, origin: &str) -> Cow<'static, str>;
```

`select_acao_value` returns the literal `"*"` only when `allow_origins == "*"`;
in every other case (force-wildcard `"**"`, single origin, list, regex match)
it echoes the request origin. `get_validated_origin` orchestrates
`CorsConfig::is_origin_allowed` (allow/deny) + `select_acao_value` (value).

This satisfies the structural intent of Fetch Living Standard §3.2
(separation of "is this origin allowed" from "what value to echo"). Behavior
is unchanged from the pre-split version.

## Decision Rule

When a review reports patterns like:

1. "`get_validated_origin` mixes validation and value selection — split it"
   → already done (the value selection is now `select_acao_value`).
2. "Add a `Cors::is_origin_allowed(&self, origin) -> bool` method"
   → **not accepted**. `CorsConfig::is_origin_allowed` already exists, is
   `pub`, and is independently unit-testable; a `Cors`-level wrapper is pure
   delegation with no value.
3. "Give `select_acao_value` a `has_credentials` parameter"
   → **not accepted**. The runtime value choice never consults credentials.
   `CorsConfig::validation_error` already hard-rejects `*`/`**` +
   `allow_credentials = true` at config-admission time (`cors.rs`), so a
   credentials branch in the data path would be dead logic, not a refactor.

Verify before closing such a re-report:

- `select_acao_value(&self, origin: &str) -> Cow<'static, str>` exists and is called
  only from `get_validated_origin`.
- `get_validated_origin` still calls `self.config.is_origin_allowed(origin)`
  directly (no `Cors::is_origin_allowed` wrapper).
- `CorsConfig::validation_error` still rejects wildcard + credentials.

## Background Notes

- Decision date: 2026-05-21
- Historical source finding (source queue removed): `tasks/todo/rfc-review/rfc-refactor-cookie-auth-02-*` (closed)
- Historical verifier (source queue removed): `tasks/todo/rfc-review/_raw/verification-a5.md → NC-A5-03` (CONFIRM)
- The finding's stale snapshot claimed the signature was
  `get_validated_origin(&self, origin: &str)`; the implementation at that time took
  `session: &mut dyn PluginSession`. The subsequent request-input reuse below
  intentionally changes it to `origin: &str`. The "two concerns mixed" observation was
  still accurate; the over-scoped wrapper + `has_credentials` parts were not.

## Resolution references

- `refactor-cookie-auth-02` — 2026-05-21.


## Borrowed validation input

The selector accepts a borrowed origin, borrows the static star for the standard
wildcard, and owns an echoed origin.
The request entry reads the canonical borrowed header accessor once for presence
and passes that same `&str` into `get_validated_origin`, which checks allowance
before copying. There is no intervening request mutation or await. Later
preflight and timing-header reads retain their existing order and behavior. Denied, absent and non-text origins produce no source
copy; a nonempty accepted echo has one owned output, while the selected star
needs no allocation. Both outputs survive subsequent session mutation and
asynchronous preflight handling without borrowing the session.
This updates the helper's ownership signature, not the allow/deny split or wire
selection rule above.
