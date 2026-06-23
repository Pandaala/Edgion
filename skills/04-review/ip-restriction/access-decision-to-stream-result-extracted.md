---
name: access-decision-to-stream-result-extracted
description: Use when reviewing stream-plugin findings of the form "the 5-arm AccessDecision → StreamPluginResult match with deny-reason literals is copy-pasted across the three stream IP-restriction plugins" — already consolidated into access_decision_to_stream_result.
status: fixed
finding_id: plugins-stream-F001
closed: 2026-05-26
---

# AccessDecision → StreamPluginResult Mapper Consolidated — FIXED

## Conclusion

Three stream IP-restriction plugins — `plugins/stream/ip_restriction/stream_ip_restriction.rs::check_ip_access`, `plugins/stream/tls_route/ip_restriction.rs::on_tls_route`, `plugins/stream/global_connection_ip_restriction/plugin.rs::on_connection` — each inlined the same 5-arm `AccessDecision → StreamPluginResult` match with identical deny-reason literals (`ip:deny:group=...`, `ip:deny:not-in-allow`, `ip:deny:default`); only the `global_connection_ip_restriction` site additionally called `record_conn_ip_restriction_event("denied")`. Consolidated into a single mapper.

Re-reports of the five-arm duplication form are **not accepted**; verify `access_decision_to_stream_result` is the single mapper before re-reporting.

## Core Rationale

**1. Why the existing free fn could not be reused**

Two underlying call types (`ConnectionIpRestrictionRules::check_ip` vs `IpRestrictionConfig::check_ip_access`) both return `AccessDecision`, so the existing canonical `check_ip_access` free fn could not be reused as-is.

**2. The consolidation**

Consolidated into `pub(crate) fn access_decision_to_stream_result(decision: AccessDecision, on_deny: impl FnOnce()) -> StreamPluginResult` in `plugins/stream/stream_plugin_trait.rs`; the no-metrics callers pass `|| {}`, the metrics caller passes `|| record_conn_ip_restriction_event("denied")`. Deny literals stay byte-identical across sites — any future format change applies in one place, downstream consumers parsing deny reasons (metrics labels, audit logs) cannot silently diverge. Pure refactor, zero behaviour change.

**3. Signature narrowed from the issue proposal**

The `impl Fn(&str)` signature proposed in the issue file was narrowed to `impl FnOnce()` — all three deny arms call with the same fixed `"denied"` label, so the label is a caller-private detail, and `FnOnce` is more accurate (each invocation fires the callback at most once).

## Fix Suggestions Not Accepted

- Re-inlining the five-arm match into any of the three sites — future format changes to deny literals must land in `access_decision_to_stream_result`.

## Re-evaluation Triggers

Re-open only if:

- The deny-reason literal contract genuinely needs to diverge per stream plugin (no such need exists today).

## Reference Cases

- Source: 2026-05-26 stream-plugin review (plugins-stream-F001).
- Distinct from `ip-restriction/deny-allow-default-algorithm-extracted-to-evaluate-ip-access.md` (plugins-stream-F012, the *pre-decision* algorithm) — F001 is the *downstream* `AccessDecision → StreamPluginResult` mapping; the boundary between the two is intentional, do not file a "consolidate into one mega-function" follow-up.
- Single mapper: `edgion-gateway/src/plugins/stream/stream_plugin_trait.rs`.
