---
name: outbound-presence-diagnostics
description: Review missing origin-field diagnostics in the shared outbound builder and its plugin consumers.
knowledge_type: regression-guards
---

# Outbound presence warnings belong to the calling plugin

## Conclusion and scope

`PresencePolicy::WarnIfMissing` omits an absent origin header, query argument,
or cookie and continues construction. `OutboundRequestDiagnostics::missing_origin`
records that observation in one stack-resident boolean. The caller consumes the
collector into its canonical PluginLog with at most one static
`warn outbound field missing` entry per logical call. It contains no field names,
values, counts, request content, or dependency detail.

`SkipIfMissing` stays silent. `Required` still stops construction, and the
consumer's existing failure policy still applies. Diagnostics neither classify
the external response nor change request bytes, retry eligibility, or health.

## Rationale and evidence

D0078 (`outbound-warn-if-missing-diagnostics`) identified that
`collect_list_fields` combined the warning and silent policies into an empty
branch. The schema and [Webhook contract](../../../01-architecture/03-controller-gateway-link/webhook.md)
already promised a PluginLog alert. This was a real observability defect, not an
accepted tradeoff or permission to add tracing.

The [logging rules](../../../03-coding/01-log-safety.md) and
[access-log ownership](../../../03-coding/observability/00-access-log.md) remain
canonical. The builder records evidence; the plugin emits it. No tracing, metric,
request-context field, or response-body log is added.

Current consumer inventory:

- `ForwardAuth::run_request` passes a fresh collector to `outbound_call_with_body`
  and emits before handling success/error. Its failure response stays fail-close.
- `WebhookKeyGet::run_request` uses a fresh collector for each configured variable.
  `resolve_webhook_key` passes it through `outbound_call`, even when returning
  `None`. Emission precedes the existing default/skip/resolved outcome.
- `WebhookCallHandle::call` exposes the collector to its caller for canonical
  emission. It currently has no production callers in this repository.
- AiGuard's evaluator and Guardrail's response processor call the same engine,
  but both resource validators prohibit every origin field. Their scratch
  collectors cannot contain presence warnings under the supported configuration
  contract. Guardrail never creates a PluginLog in the response-body callback.

## Alternatives and limits

The collector lives outside the awaited outbound future, independently of its
`Result`. A warning encountered before a later required-field, template, or
transport failure remains available. Request construction occurs before the
retry loop, so retries neither reevaluate nor repeat warnings. A health/rate/target
gate before field evaluation produces no missing-field evidence. A required field
stops evaluation; later fields are not inspected just to manufacture diagnostics.

The single bit deliberately aggregates all missing warning fields in that call.
It introduces no per-field allocations or names and is consumed by `emit` to
prevent accidental repeated emission. Multiple WebhookKeyGet variables are
separate calls; their entries remain subject to the existing total PluginLog
capacity limit. Cancellation that bypasses the owning plugin's emission point
does not guarantee a log entry.

## Verification and re-evaluation

Focused regression coverage lives in the existing inline unit-test modules:

- `presence_policies_cover_missing_and_present_fields`: all three policies and
  field kinds, both forward-all settings, and present/missing values.
- `missing_warning_is_bounded_and_survives_later_required_failure`: thousands of
  missing fields aggregate to one static entry even when construction fails later.
- `presence_warning_reaches_plugin_log_without_changing_fail_close`: real
  ForwardAuth entry, unchanged generic 503 response and fail-close outcome.
- `webhook_presence_diagnostics_follow_each_call_and_current_generation`: real
  WebhookKeyGet entry, two variables, current registry generation, default/skip,
  and the generic call handle. All exchanges fail during construction before IO.
- `status_retry_does_not_stream_oversized_error_body`: two actual attempts,
  one origin lookup, omission on both attempts, and one warning after success.

The registry test shares `I5_DISPATCH_LOCK` with full-set dispatch tests. Its
generation coverage proves a current registry lookup, not end-to-end conf_sync
hot reload. Independent review found and verified the correction of that test's
initial missing-lock race, then returned LGTM.

Fresh verification on 2026-09-10 passed:

| Command / scope | Result |
| --- | --- |
| `cargo test -p edgion-gateway --lib plugins::http::common::outbound` | 86 passed, including retry and bounded missing-warning coverage |
| `cargo test -p edgion-gateway --lib forward_auth` | 7 passed |
| `cargo test -p edgion-gateway --lib webhook` | 54 passed |
| `cargo test -p edgion-gateway --lib -- webhook_presence_diagnostics link_sys::runtime::store::tests` | 18 passed together, including shared-registry dispatch tests |
| `cargo test -p edgion-resources --lib ai_guard` / `guardrail` | 8 / 7 passed; origin restrictions remain enforced |
| `cargo fmt --all` and `cargo fmt --all -- --check` | Passed |
| `cargo check --workspace --all-targets` | Passed |
| `cargo clippy --workspace --all-targets` | Passed with existing repository warnings |
| Agent-doc, SSA force, metrics inventory, Pingora source, Gateway no-tracing, English-only guards | Passed |
| `git diff --check` and `git diff --exit-code HEAD -- edgion-tests` | Passed; all 3,264 tracked integration-harness files unchanged |

Test scopes overlap; their counts are not a unique-test total. No full integration
or full workspace test run is claimed. The independent final code review returned
LGTM after the registry-lock correction.

Re-evaluate when a new consumer uses the engine, an AI decision service permits
origin fields, request construction moves inside retries, diagnostic lifetime
changes, or PluginLog capacity/ownership changes.

## Resolution references

- D0078 — real missing-warning defect, originally deferred on 2026-09-07,
  fixed and verified on 2026-09-10. Removed the selected task
  `tasks/todo/outbound-warn-if-missing-diagnostics.md` and its duplicate campaign
  deferred pointer; this semantic page retains the reusable outcome.
