---
name: http-create-must-call-validate-and-init
description: Use when reviewing findings that HTTP `IpRestriction::create` clones the config without compiling `allow_matcher`/`deny_matcher`, leaving the ACL fail-open; documents that this was fixed (fr-types-01) so it now mirrors the stream-layer `validate_and_init` + `init_failed` fail-closed pattern.
---

# HTTP `IpRestriction::create` compiles matchers + fail-closed guard — FIXED

## Conclusion

`IpRestriction::create` (`edgion-gateway/src/plugins/http/ip_restriction/plugin.rs`) now clones
the config, calls `config.validate_and_init()` to compile `allow_matcher`/`deny_matcher`, and
stores an `init_failed` flag. `run_request` denies every request (HTTP 403, `plugin_log` =
`"ip init fail"`) when `init_failed` is set, instead of falling through to the uncompiled
default action. Fixed by `fr-types-01` (2026-05-29).

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
  passes preparse but fails matcher build at construction (would now correctly fail-closed via
  `init_failed`, but the asymmetry should be reviewed).

## Reference Cases

- `fr-types-01` — original finding, fixed 2026-05-29 (this file).
- Sibling pattern: [`stream-init-failed-pattern-duplication-not-extracted.md`](./stream-init-failed-pattern-duplication-not-extracted.md)
  (data-plane `clone → validate_and_init → init_failed` constructors).
- Source: `edgion-gateway/src/plugins/http/ip_restriction/plugin.rs` (`IpRestriction::create`, `run_request`),
  `edgion-gateway/src/plugins/stream/tls_route/ip_restriction.rs` (`TlsRouteIpRestriction::new`).
