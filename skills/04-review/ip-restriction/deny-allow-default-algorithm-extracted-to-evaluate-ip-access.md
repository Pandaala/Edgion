---
name: deny-allow-default-algorithm-extracted-to-evaluate-ip-access
description: Use when reviewing findings that propose extracting the duplicated "deny → allow → default" IP evaluation algorithm in IpRestrictionConfig::check_ip_access, ConnectionIpRestrictionRules::check_ip, and ProfileRules::check_ip into a shared free function; documents the consolidation done as plugins-stream-F012.
---

# Deny→allow→default IP evaluation algorithm — CONSOLIDATED

## Conclusion

The byte-identical body that walked `deny_matcher` → `allow_matcher` → `default_action` and returned `AccessDecision::{DenyByGroup, AllowByGroup, DenyByWhitelistMiss, Default}` was duplicated across three call sites:

- `IpRestrictionConfig::check_ip_access` (`edgion-resources/src/resources/edgion_plugins/plugin_configs/ip_restriction.rs`) — HTTP layer
- `ConnectionIpRestrictionRules::check_ip` (`edgion-resources/src/resources/edgion_stream_plugins/connection_ip_restriction_rules.rs`) — L4 stream layer
- `ProfileRules::check_ip` (`edgion-resources/src/resources/data/global_connection_ip_restriction.rs`) — cluster-scope `GlobalConnectionIpRestriction` profile

All three accept `&IpAddr` and operate on the same `Option<IpRadixMatcher>` matchers and `DefaultAction` type. Consolidated into a single free function `evaluate_ip_access(deny, allow, default_action, ip) -> AccessDecision` in `edgion-resources/src/matcher/ip_cidr_helpers.rs`. Each of the three call sites is now a 5-line delegation. Closed as `plugins-stream-F012`, 2026-05-27.

Re-reports of the form "deny → allow → default body is copy-pasted across the three IP-restriction methods" should verify `evaluate_ip_access` is still the single mapper before being filed.

## Why this collapsed cleanly (unlike sibling `plugins-stream-F003`)

This fix and the closed-as-tradeoff `plugins-stream-F003` look superficially similar — both involve duplicated bodies across HTTP and L4 IP-restriction code — but the cost/benefit verdict opposite for reasons that matter when classifying future findings:

| Axis | F-012 (consolidated) | F-003 (kept inlined) |
|---|---|---|
| Sites | 3 | 2 |
| Parameter types | Identical: `Option<&IpRadixMatcher>`, `&DefaultAction`, `&IpAddr` — already shared across the three structs | Different: `ConnectionIpRestrictionRules` vs `IpRestrictionConfig`, no shared trait |
| Abstraction overhead | 1 free fn, ~22 lines incl. doc — no trait, no generics | Required new `ValidateAndInit` trait + 2 impls + generic helper, ~15 lines just for the abstraction |
| Net code change | Net negative (~30 lines saved across the three method bodies vs ~22 lines added) | Single-digit net saving |
| Drift risk | High — algorithm is on the data plane and any priority / variant change must touch all 3 | Low — `validate_and_init` is init-only; the wrapper can't abstract over the fail-closed divergence anyway |

Rule of thumb when a "shared this duplication" finding arrives:
- If the parameter types are **already identical** and a plain free fn suffices, ship it.
- If consolidation forces a new trait/generic just to bridge type differences, treat it as a maintainability cost trade and apply `F-003`-style reasoning.

## What `evaluate_ip_access` does NOT abstract

- It does **not** subsume `access_decision_to_stream_result` (the `AccessDecision → StreamPluginResult` mapping consolidated in `plugins-stream-F001`). That function is the *stream-specific* wrapper around the `AccessDecision` returned here; the HTTP plugin still maps `AccessDecision` to an HTTP error response separately. The two refactors compose cleanly: `evaluate_ip_access` produces the decision, and each protocol's mapper turns it into a result. Do not file a follow-up "consolidate into one mega-function" finding — the boundary is intentional.
- It does **not** clone the matchers. The three call sites pass `Option<&IpRadixMatcher>` via `as_ref()`; only the final `default_action.clone()` (an enum copy, used only on the no-deny / no-allow fall-through) remains. Hot-path allocation behaviour is unchanged.
