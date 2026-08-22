---
name: mode-is-required-no-default-polarity
description: Use when reviewing request_restriction polarity findings; explains why `mode` is an explicit required field with no default, why the old `Blacklist` default was the actual defect, and why allow/deny rule-group lists were considered and rejected.
status: fixed
finding_id: request-restriction-01
closed: 2026-08-22
---

# `request_restriction` polarity: explicit required `mode`, no default — FIXED

## Conclusion

`RequestRestrictionConfig` keeps a single global polarity field and a single
`ruleGroups` list. Two things changed: the values are now `Deny` / `Allow` (was
`Blacklist` / `Whitelist`), and the field is **required** — a config with no
`mode` is rejected by `check_fields()`.

The defect was never "a global polarity field exists". It was **the default
value**. Fix suggestions of the form "give `mode` a sensible default so simple
configs are shorter" are **not accepted**.

## Core Rationale

**1. A defaulted polarity is a silent fail-open**

`mode` used to default to `Blacklist`. An operator writing a whitelist who
omitted the `mode:` line got a blacklist instead — and a blacklist whose groups
match nothing **passes every request**. A policy intended to fail closed
silently failed open, with a YAML document that looked entirely normal and no
warning anywhere. Making the field required removes the failure mode outright:
the polarity is always written down, so it can never be inferred wrongly.

Modelling it as `Option<RestrictionMode>` + a `check_fields` branch (rather than
a serde-required field) keeps the error on the normal plugin-validation path, so
a missing `mode` produces a readable `Accepted=False` reason instead of failing
the whole EdgionPlugins resource's deserialization.

This covers a *missing* `mode` only. A *stale value* (`Blacklist` / `Whitelist`)
is still a serde unknown-variant error and so fails the resource's
deserialization — `RestrictionMode` deliberately carries no `#[serde(other)]`
fallback, because silently mapping an unrecognised polarity onto some default is
the exact hazard this entry is about. The serde message names both valid
variants, and pre-release there is no back-compat burden.

**2. An explicit field beats inferred polarity for readability**

A two-list shape (`denyRuleGroups` / `allowRuleGroups`, polarity = list
membership) was implemented twice and reverted twice. It is exactly as
expressive for a single-polarity policy, but it makes the fallback verdict
implicit: `allowRuleGroups` never states that unmatched traffic is rejected, the
reader has to know the convention. An explicit `mode` states the polarity as a
declaration, which is what an operator reading a security policy wants. The
locality argument for the two-list shape (polarity sits on the key that owns the
groups) did not outweigh this.

**3. `Deny` / `Allow` over `Blacklist` / `Whitelist`**

Aligns with AGENTS.md cross-cutting rule 11's vocabulary and with
`DefaultAction::Allow | Deny` in `ip_restriction` / `geoip_location`, so the
access-control family reads with one set of terms.

## Fix Suggestions Not Accepted

- Give `mode` a default (`Deny`, `Blacklist`, or anything else) — reintroduces the silent fail-open in rationale 1. Shorter configs are not worth it.
- Split into `allowRuleGroups` / `denyRuleGroups` and drop `mode` — implemented and reverted; see rationale 2. Equivalent expressiveness for one polarity, implicit fallback verdict.
- Add `denyRuleGroups` + `allowRuleGroups` + `defaultAction` with deny-wins so "allow X except Y" is expressible in one plugin — also implemented and reverted. It works and is a defensible shape, but it costs an extra required field in every configuration to buy a case that is currently served by a regex exclusion or by chaining two plugin instances. Revisit only under the trigger below, not as a general cleanup.
- Add a `defaultAction` field alongside `mode` — redundant: with one `ruleGroups` list and one `mode`, the fallback verdict is already fully determined (`Allow` => unmatched denied, `Deny` => unmatched passes). It would only ever hold its one correct value.
- Add per-group or per-rule polarity — that is the nginx `ngx_http_access_module` model (single ordered list, first match wins, order-sensitive). Edgion deliberately follows the order-insensitive Envoy/NetworkPolicy shape instead.
- "Fix" the all-`Skip` foot-gun (a `mode: Allow` group whose predicates are all `Skip`-dropped rejects every request) — documented behavior of `onMissing: Skip`, regression-tested in both polarities; the guidance is to use `NoMatch`/`Match` when the source may be absent.

## Re-evaluation Triggers

Re-open this decision only if:

- Real configurations repeatedly need allow **and** deny simultaneously ("allow /api/*, except /api/admin") and the regex-exclusion / two-plugin-chain workarounds prove inadequate. A single `mode` cannot describe that; the shape to adopt is `denyRuleGroups` + `allowRuleGroups` + a required `defaultAction`, deny-first, mirroring `evaluate_ip_access_multi`'s precedence.
- Per-group rejection status codes / messages are needed, which would move `status` / `message` down a level and may reopen the group-shape question.

## Reference Cases

- Session of 2026-08-22 closing the "replace `mode`" task; the task file was removed on close.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/request_restriction.rs` — `RestrictionMode`, `check_fields`, `allow_mode`
- `edgion-gateway/src/plugins/http/request_restriction/plugin.rs` — `evaluate_groups`, `run_request`
- `edgion-resources/src/matcher/ip_cidr_helpers.rs` — `evaluate_ip_access_multi`, the deny-first algorithm to adopt if both directions are ever supported
- `docs/en/user-guide/http-route/filters/edgion-plugins/request-restriction.md`
