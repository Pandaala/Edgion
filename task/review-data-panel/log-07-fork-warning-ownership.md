# Decide inherited proxy warning ownership

Status: deferred for user decision. No runtime change.
Source baseline: `ba77ed95c`; Pingora fork
`1aaa45cc70430bc782990bc20ddee3fef36d6b99`.

## Evidence

EdgionHttpProxy inherits suppress_proxy_warn_log=false. The fork has eleven
production call sites: one in lib.rs for UpstreamRetry, and ten DownstreamCache
sites split across proxy_h1.rs (three), proxy_h2.rs (four) and proxy_custom.rs
(three). Each checks the hook before its log::warn! invocation. This hook is
separate from final-error suppress_error_log, which Edgion overrides for already
written local replies.

The retry warning follows response-head and request-replay vetoes and requires
error.retry(). It includes the error, attempt counter and request_summary. This
is a retryable-attempt warning, not proof another attempt runs: the loop's
max-retries condition is tested again on the next iteration. Cache warnings
report ignored downstream errors while cache fill continues. Their exact current
Edgion reachability, especially custom connectors, is not established here.

The binary's common logging bootstrap independently rejects sub-ERROR events for
pingora_proxy::proxy_h1 and its descendants. It does not apply that floor to root
pingora_proxy, proxy_h2 or proxy_custom. Broader operator filtering still applies.
Do not mistake the H1 emission filter for absence of argument construction:
locked log 0.4.33 checks STATIC_MAX_LEVEL and the global max_level before evaluating
format_args arguments, including request_summary(). The tracing-log 0.2 bridge
checks target enablement afterward. With the macro's level gates open, a rejected
H1 target can therefore still construct the summary String. Error Display itself
is deferred by format_args; do not equate it with eager String construction.

Edgion's summary first obtains the transport summary and then appends trace ID.
Connect-failure, per-try-timeout and connection-reuse paths already record selected
attempt reasons. This does not establish full diagnostic equivalence with the
fork warnings. Pingora-internal Decided(true) retries are intentionally not all
reason-labeled, per the existing retry-observability decision.

## Decision

Assess a selective warning-hook policy against the binary-owned access-log
contract, retaining necessary diagnostics in their canonical owner. Establish
which facts are already represented and which are intentionally omitted before
suppressing events. Simply returning true everywhere changes diagnostic behavior;
it is not a behavior-neutral allocation refactor. Keeping current output during
this decision is not a finding that all inherited warnings satisfy project rules.

Do not modify inherited fork statements or broaden the shared system-log filter
without a separately reviewed scope. Do not introduce raw error/summary copies
into access logs, another event channel, or per-retry counters: existing policy
explicitly rejects those counters in favor of access-log aggregation. Preserve
redaction and avoid duplicating attempt records. No performance benchmark or
unconditional per-request saving is claimed.

## Acceptance before implementation

Use IMPL with a frozen allow-list and independent reviews. Map both warning
contexts to call sites and existing access-log evidence; handle the non-exhaustive
context enum deliberately on fork upgrades. Prove actual cache/custom-path
reachability before promising coverage there. Preserve final-error suppression,
retry eligibility/counts, replay vetoes, response-head closure, cache-fill state,
on_proxy_failure notification and terminal access-log publication.

Use captured logging tests with macro global levels open/closed and target
filters enabled/disabled. Count summary construction separately from emitted
messages and error Display work. Validate no lost required attempt evidence for
connect failure, timeout, reuse, internally decided retries and exhausted attempt
loops; do not infer another attempt from a warning. Cover downstream-cache errors
where reachable without enabling fork caching merely for production parity.
Measure CPU/allocation effects separately from changed log volume and sink IO.
Do not add a new request-path log to demonstrate that suppression worked.

## Sources

- Gateway `routes/http/proxy_http/mod.rs`, pg_fail_to_connect.rs and
  pg_error_while_proxy.rs; terminal pg_logging.rs and AccessLogEntry are the
  canonical observation owners to revalidate before implementation.
- Locked fork `pingora-proxy/src/{lib,proxy_h1,proxy_h2,proxy_custom,proxy_trait}.rs`.
- Locked log 0.4.33 `src/{macros,__private_api}.rs` and tracing-log 0.2.0
  `src/log_tracer.rs`.
- `edgion-common/src/observe/logs.rs` and `tests/unit/observe/logs_test.rs`.
- `skills/03-coding/01-log-safety.md`.
- `skills/04-review/observability/regression-guards/pingora-h1-header-logs-have-an-error-floor.md`.
- `skills/04-review/observability/intentional-design/retry-counter-metric-declined.md`.
