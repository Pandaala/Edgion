---
name: stream-preparse-uses-plugin-validation
description: Use when reviewing findings about `ConnectionIpRestrictionRules` (or any L4 stream-plugin config) using a special-cased `clone().validate_and_init()` at controller preparse time instead of the standard `PluginValidation::validation_error()` pattern; documents that this was fixed (plugins-stream-F009) and the standard is now uniform with HTTP plugins.
---

# Stream-plugin controller preparse uses `PluginValidation` — FIXED

## Conclusion

`ConnectionIpRestrictionRules` implements `PluginValidation` (`validation_error(&self) -> Option<String>`, stateless, built on `validate_groups_structure`). The Controller's `EdgionStreamPluginsHandler::preparse` calls `rules.validation_error()` on the Stage 1 `ConnectionFilter` (L4) plugins — the same shape used by HTTP plugins. The earlier `rules.clone().validate_and_init()` workaround (which built the radix matcher only to drop it) was removed by `plugins-stream-F009` (2026-05-27).

Fix suggestions of the form "ConnectionIpRestrictionRules should implement PluginValidation; the controller should not call `clone().validate_and_init()`" are **already done** — do not re-report.

## Core Rationale

**1. The split between `validate_and_init` and `validation_error` is intentional**

`validate_and_init(&mut self)` is the data-plane initializer: it validates AND builds the `IpRadixMatcher`s into the private `allow_matcher` / `deny_matcher` fields. It is called once at plugin construction (`StreamIpRestriction::new`, `TlsRouteIpRestriction::new`). It owns the matcher build, so it must stay `&mut self` and stay side-effectful.

`validation_error(&self) -> Option<String>` is the controller-preparse view: it runs the SAME structural checks (`validate_groups_structure(groups)` + both-empty rejection) without building or storing matchers. The CR status path needs only to surface `Accepted=False` with the error message, never the parsed matcher. Calling `validate_and_init` on a clone for this purpose worked but was misaligned with the HTTP-plugin preparse pattern and wasted one `IpRadixMatcher` build per CR generation.

**2. Error-set parity is preserved**

`build_grouped_matcher` is defined as `validate_groups_structure(groups)? + matcher build`. The matcher-build errors not covered by `validate_groups_structure` (`IpRadixTreeError::TooManyNodes` / `TooManyGroups` / `EmptyTree`) are capacity bounds (>65534 groups or >u32::MAX tree nodes) that are not reachable through K8s-sized CRDs; the empty-groups case is caught upstream by the both-lists-empty check. So `validation_error()` and the old `clone().validate_and_init()` surface the same operator-realistic error set, with the same raw message text.

**3. Status-condition contract intact**

The preparse wrapper string stayed identical: `"plugins[{idx}] (IpRestriction): {e}"`. `LEGACY_SCHEMA_MARKER` detection (for `LegacySchemaDeprecated` reason mapping) is unaffected because legacy errors take the early-`continue` path before structural validation runs. Existing tests covering both the structural-error → `Invalid` reason mapping and the legacy → `LegacySchemaDeprecated` mapping continue to hold.

**4. Stage 2 (TlsRoute / HTTP-layer `IpRestrictionConfig`) deliberately unchanged**

`TlsRouteStreamPlugin::IpRestriction` carries the HTTP-layer `IpRestrictionConfig`, which already has its own `PluginValidation` impl in `types/resources/edgion_plugins/plugin_configs/ip_restriction.rs`. The Stage 2 preparse call still uses `config.clone().validate_and_init()` because that code path was outside the F-009 scope (and switching it is its own minimal-touch finding, not part of this entry). Do not bundle a Stage 2 switch into a Stage 1 finding.

**5. Data-plane sites still use `validate_and_init`**

`StreamIpRestriction::new` and `TlsRouteIpRestriction::new` keep their `cfg.clone(); cfg.validate_and_init()` shape — they NEED the matcher side-effect. The pattern duplication between those two constructors is a separate, deliberately-declined extraction (see `stream-init-failed-pattern-duplication-not-extracted.md`). The F-009 fix did NOT change the data-plane sites and must not be conflated with that one.

## Fix Suggestions Not Accepted

- "ConnectionIpRestrictionRules is missing `PluginValidation`" — already implemented; verify `edgion-resources/src/resources/edgion_stream_plugins/connection_ip_restriction_rules.rs` before filing.
- "Controller preparse uses non-standard `clone().validate_and_init()` for L4 ip-restriction" — Stage 1 now calls `validation_error()`; confirm by reading the `EdgionStreamPlugin::IpRestriction` arm in `EdgionStreamPluginsHandler::preparse`.
- "Replace Stage 2 TlsRoute `clone().validate_and_init()` too" — out of scope for F-009; file as a separate finding if surfaced. Stage 2's `IpRestrictionConfig` already has a `PluginValidation` impl, so the switch would be one-line, but it does not belong inside an F-009 re-report.

## Re-evaluation Triggers

Re-open only if:

- A new L4 stream-plugin config type is added that depends on a side-effectful `validate_and_init` at controller preparse time (i.e., regresses to the old pattern).
- `validate_groups_structure` and `build_grouped_matcher` diverge such that an operator-reachable invalid config slips past preparse but trips the data-plane builder (would be a fail-closed silent-blackhole-during-construction regression).
- A real fail-closed contract change ships and the controller status text needs new prefixes that the data-plane `validate_and_init` does not produce.

## Reference Cases

- `plugins-stream-F009` — original finding, fixed 2026-05-27 (this file).
- `tasks/gateway-deep-review/plugins-stream/INDEX.md` row 23.
- Sibling design note: [`stream-init-failed-pattern-duplication-not-extracted.md`](./stream-init-failed-pattern-duplication-not-extracted.md) — data-plane `validate_and_init` duplication is deliberately kept; do not conflate.
- Source: `edgion-resources/src/resources/edgion_stream_plugins/connection_ip_restriction_rules.rs` (`impl PluginValidation`), `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_stream_plugins.rs` (preparse Stage 1).
