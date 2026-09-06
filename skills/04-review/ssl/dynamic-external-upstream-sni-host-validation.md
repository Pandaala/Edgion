---
name: dynamic-external-upstream-sni-host-validation
description: Use when reviewing DynamicExternalUpstream findings that explicit empty SNI disables upstream certificate verification, or that overrideHost is not validated before publication.
status: fixed
finding_id: D0045
closed: 2026-09-05
---

# DynamicExternalUpstream rejects unsafe explicit SNI and Host overrides — FIXED (D0045)

## Conclusion

Shared Controller/Gateway validation now rejects an explicit empty or illegal
`sni`, and an explicit empty, non-header, or non-authority `overrideHost`,
before a DynamicExternalUpstream generation can be published. Omitted fields
keep the existing defaults: SNI falls back to `domain`, and Host stays the
original request Host. No new configuration fields or knobs were added.

Fix suggestions of the form "treat an explicit empty SNI as absent at request
time", "change Pingora empty-SNI verification", or "make `sni` /
`overrideHost` required" are **not accepted**.

## Core Rationale

**1. Publication is the correct owner**

`effective_sni()` uses `Option::unwrap_or`, so an explicit empty string is a
real override and reaches `ExternalJumpPreset`. The supported Pingora
BoringSSL connector disables SNI and certificate verification for an empty
peer SNI. Rejecting that value in `check_fields()` / `validation_error()`
keeps Controller and Gateway enforcement on the same path.

**2. Optional fields stay optional**

Both fields remain `Option<String>` with serde default/skip. Operators who
omit them get the previous runtime defaults. Validation runs only when a
value is present.

**3. Shared predicates, no accidental narrowing**

Nonempty explicit SNI reuses the existing `domain` character predicate so
internal names and supported IP forms stay valid. `overrideHost` requires a
valid HTTP header value and an HTTP Host authority: `uri-host` with an
optional numeric port in `1..=65535` using ASCII digits only. URI
userinfo (`user@host`), signed ports (`+80`), and out-of-range ports are
rejected even though `http::uri::Authority` accepts userinfo and Rust
integer parsing accepts a leading `+`. Error strings are fixed and do
not echo configured values.

## Fix Suggestions Not Accepted

- Coalescing empty SNI to `domain` at runtime — hides the unsafe config and
  leaves the next publisher unprotected.
- Changing Pingora empty-SNI TLS behavior — out of scope; the product
  contract is to refuse the unsafe config.
- Requiring `sni` or `overrideHost`, or adding a strictness knob — both
  fields are optional overrides; the default is still "do not set them".
- Using `is_valid_domain()` for SNI — that helper rejects single-label
  internal names such as `localhost` that `domain` already accepts.
- Changing Gateway runtime, schema field names, or existing integration
  tests.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora stops disabling verification on empty peer SNI, or Edgion stops
  using that connector path.
- A supported `domain` / SNI name form is later forbidden by an approved
  contract.
- `overrideHost` gains a different wire encoding than HTTP Host.

## Reference Cases

- D0045 / B0073 deferred finding
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/dynamic_external_upstream.rs`
- `docs/en/user-guide/http-route/filters/edgion-plugins/dynamic-upstream.md`
