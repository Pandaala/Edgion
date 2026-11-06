# res3-02: GeoIpLocation accepts unknown country/continent codes, so a mistyped deny rule silently never matches

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/geo_rule.rs:279-298` (`GeoRule::field_error`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/geo_rule.rs:303-338` (`GeoRule::compile`: unknown codes only `tracing::warn!`)
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/geoip_location.rs:127-156,267-280` (`check_fields` / `PluginValidation`)
  - `edgion-resources/src/geoip_codes.rs:1-5,46-53` (tables that exist "so the controller can validate country codes at resource ingest")

## Summary
`GeoIpLocation` deny and allow rules match exact, upper-cased ISO-3166 / continent codes. The
Controller's admission path (`check_fields`) never checks the values against the code tables
that exist in this crate for exactly that purpose. The only check is inside `compile()` on the
Gateway, and it logs a `tracing::warn!` and keeps the rule. A typo or a non-ISO code is therefore
accepted with `Accepted=True` and never matches. On a deny list that means the block is silently
not enforced.

## Current behavior (evidence)
- `GeoRule::field_error` (`geo_rule.rs:279-298`) only checks unsupported fields, source/stream
  compatibility, CIDR-vs-values shape and that `values` is non-empty.
- `GeoIpLocationConfig::check_fields` → `GeoRuleGroup::check_fields` → `field_error`. This is the
  whole `PluginValidation::validation_error` (`geoip_location.rs:278-280`). The Controller calls
  this to set status.
- `GeoRule::compile` (`geo_rule.rs:309-327`) upper-cases the values and calls
  `geoip_codes::is_valid_country` / `is_valid_continent`. On a miss it emits
  `tracing::warn!("... is not a known country code")` and continues.
- `geoip_codes.rs:1-5` says the tables live in `edgion-resources` "so the controller can validate
  country codes at resource ingest". No Controller or resource validation path calls them; the only
  call sites are the two warnings above.
- `dimension_matches` (`geo_rule.rs:340-345`) is an exact string compare with the MMDB value. So
  `UK` (common, but not ISO; MaxMind emits `GB`), `EN`, `XX`, or a three-letter `USA` can never match.

Example: `denyRuleGroups: [{rules: [{source: RemoteIp, key: country, values: [UK, RU]}]}]` blocks
Russia and lets the UK through. Status reports the plugin as accepted. A Gateway WARN, which nobody
reads at apply time, is the only signal.

## Industry comparison
- Cloudflare WAF / Firewall rules reject an unknown `ip.geoip.country` value when the rule is saved.
- AWS WAF `GeoMatchStatement` validates `CountryCodes` against an ISO-3166 enum at API time.
- NGINX `geo`/`map` has no enum, but Edgion already ships the table and documents the intent to
  validate at ingest.

## Impact
Geo-based deny lists are a common compliance or sanctions control. A single invalid code disables
part of the control. Nothing in status shows this, so the operator believes it is enforced. The same
applies to `continent`. Allow lists fail the other way: the region is blocked, which is at least
visible.

## Suggested direction
- In `GeoRule::field_error`, reject `country` values that fail `is_valid_country(&v.to_ascii_uppercase())`
  and `continent` values that fail `is_valid_continent(...)`. Use the indexed message format the
  other rule errors use. The Controller then reports `Accepted=False`, and Controller and Gateway
  agree.
- Remove the `tracing::warn!` branch in `compile()`, since it is unreachable once admission rejects.
- `subdivision`/`city` cannot be validated statically. Document them as exact MMDB values (case-folded
  for subdivision) in the feature schema.

## Notes
- Checked `skills/04-review/plugin-runtime/regression-guards/invalid-stream-geoip-generation-fail-closed.md`
  and the observability GeoIP pages. They cover unknown or removed *fields* (fail-closed) and lookup
  failure policy, not unknown code *values*. No intentional-design page accepts this warning-only
  behavior.
- Pre-release: rejecting previously accepted invalid codes needs no compatibility shim.

## Verification (independent)

Verdict: CONFIRMED. Severity P2 kept.
- `GeoRule::field_error` (`geo_rule.rs:279-298`) checks shape only; `compile` (`:303-338`) explicitly documents "WARN (never reject)" and only `tracing::warn!`s on unknown codes.
- `is_valid_country`/`is_valid_continent` have no callers outside `compile` and unit tests (grep), contradicting the `geoip_codes.rs:1-5` module doc that they exist for Controller ingest validation.
- `dimension_matches` (`geo_rule.rs:340-345`) is an exact compare, so `UK`/`USA` never match; no review page accepts warning-only behavior for code values.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `geo_rule.rs`, `geoip_location.rs` and `geoip_codes.rs` are unchanged; `is_valid_country`/`is_valid_continent` are still called only from the warn-only branch in `GeoRule::compile` (`geo_rule.rs:314,324`).
