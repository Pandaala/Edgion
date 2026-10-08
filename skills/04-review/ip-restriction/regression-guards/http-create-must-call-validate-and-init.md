---
name: http-create-must-call-validate-and-init
description: Use when reviewing findings that HTTP `IpRestriction::create` clones the config without compiling `allow_matcher`/`deny_matcher`, leaving the ACL fail-open; documents that this was fixed (fr-types-01) so it now mirrors the stream-layer `validate_and_init` + `init_failed` fail-closed pattern; also documents the 2026-06-20 partial-good update.
knowledge_type: regression-guards
---

# HTTP `IpRestriction::create` compiles matchers + fail-closed guard — FIXED

## Conclusion

`IpRestriction::create_bound` (`edgion-gateway/src/plugins/http/ip_restriction/plugin.rs`) clones
the config, calls `config.validate_and_init()` to compile `allow_matcher`/`deny_matcher`, and
stores an `init_failed` flag. `run_request` denies every request (HTTP 403, `plugin_log` =
`"ip init fail"`) when `init_failed` is set, instead of falling through to the uncompiled
default action. Fixed by `fr-types-01` (2026-05-29).

**Partial-good update (2026-06-20):** `validate_and_init` now uses `build_grouped_matcher_partial`
for inline `allow`/`deny` group compilation. Individual bad CIDR entries are **skipped** (not
fatal), so a bad CIDR no longer triggers `init_failed`. The plugin loads with the valid entries.
`init_failed` is now only set by structural failures: both `allow` and `deny` absent/empty with
no `allowRefs`/`denyRefs`, or an invalid final `status` code (200–599 range). Bad entries surface as
`PartiallyInvalid` status condition + `edgion_ip_deny_entries_skipped_total{scope="http"}` metric
(for skipped deny entries). Fix suggestions of the form "bad inline CIDR drives HTTP IpRestriction
to `init_failed`/deny-all" are **no longer correct** — verify the `build_grouped_matcher_partial`
call inside `validate_and_init` before re-reporting.

Fix suggestions of the form "HTTP `IpRestriction::create` never calls `validate_and_init`, so
`allow_matcher`/`deny_matcher` stay `None` and an allow-only whitelist silently allows every
IP (fail-open ACL)" are **already done** — do not re-report.

## Core Rationale

**1. The defect was constructor-only, not evaluator**

`allow_matcher`/`deny_matcher` carry `#[serde(skip)]`, so they are `None` after the
gRPC/YAML → preparse → runtime path. Only `validate_and_init` (`ip_restriction.rs`) builds
them via `build_grouped_matcher`. The old `create` did `config: config.clone()` with no
init call, so the live `IpRestriction` held `None` matchers; `evaluate_ip_access` skipped
both deny and allow branches and returned `Default(DefaultAction::Allow)` — every IP allowed.
`validation_error()` at preparse only checks CIDR syntax (it does **not** build matchers), so
`seal_fail_closed` never fired for a syntactically-valid config. The earlier `review-2` pass
cleared ip_restriction by testing the evaluator with matchers already built via
`IpRestrictionConfig::new` in unit tests — that path was correct in isolation; the defect was
the constructor used by the live path.

**2. The fix mirrors the established stream-layer pattern**

`TlsRouteIpRestriction::new` and `StreamIpRestriction::new` already clone + `validate_and_init`
+ set `init_failed` (deny-all on failure). The HTTP plugin was the outlier; it now follows the
same shape. The constructor-side `tracing::error!` is acceptable: `create` runs at plugin
runtime build (config-apply / hot-reload), off the request path, identical to the stream
constructors — it does not violate the data-plane no-tracing rule.

**3. Regression is locked at the constructor→evaluator boundary**

`tests::test_create_compiles_matchers_whitelist_enforced` builds the config the way
deserialization yields it (matchers `None`, **not** routed through `IpRestrictionConfig::new`),
calls `IpRestriction::create`, and asserts an out-of-allow-list IP is denied
(`deny:not-in-allow`). A test that constructs the config via `IpRestrictionConfig::new` would
pre-build the matchers and mask the bug — re-reports must verify the matcher-`None` path.

## Fix Suggestions Not Accepted

- "HTTP IpRestriction is fail-open / missing `validate_and_init` in `create`" — already fixed;
  verify the current `create` clones + calls `validate_and_init` + sets `init_failed` before filing.
- "Add a generic `validate_and_init` call inside `create_request_filter_from_edgion` instead" —
  the per-plugin fix was chosen for consistency with `RequestRestriction::create` and the stream
  constructors; a generic dispatch-layer init is not required.

## Re-evaluation Triggers

Re-open only if:

- A new field with `#[serde(skip)]` runtime state is added to `IpRestrictionConfig` and `create`
  is not updated to populate it.
- `validate_and_init` and `validation_error` diverge such that an operator-reachable config
  passes preparse but causes a structural failure at construction (would now correctly fail-closed
  via `init_failed`, but the asymmetry should be reviewed).
- The partial-good semantics change (e.g., a bad CIDR becomes fatal again) — in that case the
  `build_grouped_matcher_partial` call inside `validate_and_init` must be reverted to
  `build_grouped_matcher` and `init_failed` must be updated accordingly.

## Reference Cases

- `fr-types-01` — original finding, fixed 2026-05-29 (this file).
- Sibling pattern: [`stream-init-failed-pattern-duplication-not-extracted.md`](../intentional-design/stream-init-failed-pattern-duplication-not-extracted.md)
  (data-plane `clone → validate_and_init → init_failed` constructors).
- Source: `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs` (`IpRestriction::create`, `run_request`),
  `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs` (`TlsRouteIpRestriction::new`).
- Final-status validation: [`request-local-reply-final-status-validation.md`](../../resources/regression-guards/request-local-reply-final-status-validation.md).

## Owning constructor boundary (R5c2)

The production entry point is now `create_bound(HttpIpListEntry)`. It validates the
reference-bearing owner separately from target slots, then preserves the existing
config clone and `validate_and_init` path described above. Canonically invalid
external targets contribute no runtime key; authored allow mode and inline/permitted
siblings remain effective, and deny-only failed sets are dropped. The old detached
`create(config, namespace)` spelling exists only as a fixture wrapper in the mirrored
test module and delegates through an actual fixture owner. Stream, TLS-route and GCIR
constructors use the corresponding owning loans; no request-time parsing was added.
