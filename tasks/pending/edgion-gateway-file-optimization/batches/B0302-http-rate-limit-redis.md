# B0302 — HTTP Redis Rate Limit Plugin

## Scope

- Baseline: `e62956dc`.
- Pipeline state: terminal; changed and committed after independent approval and all gates.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- Reopened after external `264847df`; B0097 remains historical evidence only.
- The analyst must inspect the full Redis rate-limit plugin, shared LinkSys Redis and rate-limiter boundaries, configuration preparation, hot-path behavior, failure policy, observability ownership and directly related tests before deciding change, examined-no-change or defer.

## Analysis result

- Analyst `b0302_rate_limit_redis_analysis` revalidated the full module, PluginRuntime construction, LinkSys Redis/shared limiter core, configuration, hot-path failure policy, observability and tests at `e62956dc`.
- `Retry-After` correctly rounds a positive millisecond delay upward, but `X-RateLimit-Reset` floors both wall time and retry duration. Normal sub-second denials can therefore advertise the current or past Unix second while requiring a one-second retry delay.
- Decision: change, simple. Exact technical allow-list is only `edgion-gateway/src/plugins/http/rate_limit_redis/plugin.rs`, including colocated deterministic tests.
- Add a private pure absolute-deadline calculation with checked time arithmetic and ceiling Unix-second conversion; omit reset headers only when the time is unrepresentable. Preserve all Redis, failure-policy, response, Dye, PluginLog and header-gating behavior.
- Tests cover sub-second, fractional-current-second, exact-second and unrepresentable/pre-epoch inputs. Existing D0028/D0046/D0063 and sibling local-rate-limit arithmetic remain separate.

## Implementation

- Implementer `b0302_rate_limit_redis_implement` added a private checked absolute-deadline helper and uses it for default/custom reset headers. Fractional deadlines round upward; pre-epoch or overflowing deadlines omit only the reset header.
- Existing `Retry-After`, header gating, response/status/write ownership, keep-alive, Redis/LinkSys/shared-core/key, Dye, PluginLog and failure-policy behavior are unchanged.
- Five deterministic pure tests cover sub-second, fractional-current-second, exact-second, pre-epoch and overflow cases; the focused module suite passed 19/19.

## Independent review

- Reviewer `b0302_rate_limit_redis_review`: LGTM with no Must Fix, Should Fix or new deferred finding.
- The reviewer confirmed consistent `ceil(retry)`/`ceil(now + retry)` semantics, checked u64 arithmetic, conditional reset omission, exact scope, preserved cross-cutting ownership and proportionate value.

## Verification

- Focused Redis rate-limit tests passed 19/19; formatting and diff checks passed.
- Formatting, locked workspace check, locked workspace Clippy (existing warnings, exit 0), agent-doc, SSA, metrics, Pingora source, Gateway tracing, English-only, frozen-integration and diff gates passed at `e62956dc`.
- Frozen integration files were not modified.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `8ae7f72f`.
