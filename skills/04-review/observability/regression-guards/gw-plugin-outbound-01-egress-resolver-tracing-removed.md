---
name: gw-plugin-outbound-01-egress-resolver-tracing-removed
description: Use when reviewing findings that EgressResolver should emit a tracing warning for blocked DNS addresses, or when considering reintroducing per-address tracing in the shared ForwardAuth/Webhook outbound resolver.
status: fixed
finding_id: gw-plugin-outbound-01
closed: 2026-08-02
knowledge_type: regression-guards
---

# EgressResolver blocked-address tracing is removed — FIXED

## Conclusion

`EgressResolver::filter_addrs` must remain tracing-free. It runs from
`reqwest::dns::Resolve::resolve` on the ForwardAuth and Webhook outbound request
path, so blocked addresses are reported only through the existing
`record_egress_blocked(reason)` gateway metric. The per-address `tracing::warn!`
was removed on 2026-08-02, and `plugins/http/common/outbound/` was added to the
data-plane tracing scanner.

Fix suggestions of the form "keep the warning behind the resolver TTL",
"downgrade it to `tracing::debug!`", or "emit it once per host" are **not
accepted**. They still put a `tracing::*` call on a request/connection path.

## Core Rationale

**1. DNS rejection is request-path work, not a cold diagnostic callback.**

The actual-owner managed client builder installs `EgressResolver` for URL targets.
`ForwardAuth::run_request` and Webhook outbound calls execute the resulting
`reqwest::Client`; on a resolver-cache miss, `Resolve::resolve` calls
`filter_addrs` before allowing the connection. A blocked DNS answer therefore
reaches the function during live request processing even though the resolver
object itself was constructed during config assembly.

**2. The metric already carries the bounded operational signal.**

`filter_addrs` preserves the exact address classification and increments
`edgion_outbound_egress_blocked_total{reason=...}` once for each rejected
address. The all-blocked result still returns a resolver error, so callers keep
their existing failure behavior. Removing the tracing event changes neither
SSRF enforcement nor metric behavior.

**3. The scanner and the written boundary now agree.**

The canonical data-plane entry list in `skills/03-coding/01-log-safety.md` now
includes `plugins/http/common/outbound/`, and
`cicd/checks/check_gateway_no_tracing.py` scans that directory recursively.
The startup-only `configure_egress_defaults` info log remains allowed by the
scanner's constructor/configuration classification; request-time outbound
helpers must stay tracing-free.

## Fix Suggestions Not Accepted

- Re-add the warning behind `trace_rejections`, a TTL, a `OnceLock`, or a host
  latch — frequency does not change the Iron Rule 3 path classification.
- Downgrade the event to `debug!`/`trace!` or use a separate tracing target — the
  prohibition covers every `tracing::*` level and target on the hot path.
- Add a new per-address metric label or a new access-log field to recover the
  host/address detail — the existing bounded reason counter is sufficient, and
  host/IP values would add cardinality and sensitive topology detail without a
  demonstrated operational requirement.

## Re-evaluation Triggers

Re-open only if:

- A new cold-only caller needs a diagnostic that cannot be represented by the
  existing metric or caller-level error channel; such a signal must be emitted
  by that cold caller, not by `filter_addrs`.
- The egress metric is removed or no longer records the rejection reason needed
  by the outbound SSRF runbook.
- The outbound resolver is moved off the request/connection path and the
  canonical data-plane boundary is updated with evidence of that lifecycle
  change.

## Reference Cases

- Source finding: `tasks/fable-review/gw-plugin-outbound-01-egress-resolver-tracing-on-data-plane-hot-path.md`.
- Iron Rule 3: `skills/03-coding/01-log-safety.md`.
- Scanner: `cicd/checks/check_gateway_no_tracing.py`.
- Related fixed decisions: `observability/oidc-outbound-san-mismatch-tracing-removed.md`,
  `observability/oidc-jwks-provider-tracing-relocated.md`, and
  `observability/wasm-request-path-tracing-removed.md`.

## Resolution references

- `gw-plugin-outbound-01` — 2026-08-02.
