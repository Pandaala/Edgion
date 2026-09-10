# B0297 — HTTP OIDC Request Filter

- Pipeline state: terminal; changed after corrected/approved plan, independent final LGTM and full verification.
- Active anchor: `edgion-gateway`; baseline `9963a3da`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/request_filter_impl.rs`, reopened after external `74e6ccdd`; historical B0093 is evidence to revalidate.
- Analyst must audit the complete request authentication/refresh/introspection/callback dispatch and terminal-response ownership, including the external changes and all relevant OIDC/session/JWKS/error consumers. B0295's JWKS publication deadline fix is complete; D0058 owns dependency-error redaction.
- Existing `edgion-tests/**`, user-owned protected paths, campaign records and all technical files are read-only during analysis. No `fix-issue-cn`, commit or push.

## Analysis result

- Fresh child analyst `b0297_request_filter_analysis` audited the full target/OIDC module, request/callback/session/token/refresh/JWKS/error and terminal-response call chains, external diff, unit/frozen integration coverage and review tasks at `9963a3da`.
- Confirmed defect: a callback-path request without nonempty `code` and `state`, including an IdP `error=access_denied` response, falls through ordinary unauthenticated handling and can redirect to the IdP again under default `UnauthAction::Auth`, creating an authentication loop.
- Decision: change, plan-first authentication state-machine fix. Exact technical allow-list: `request_filter_impl.rs` and `tests.rs`.
- Callback requests missing either required value must use existing `send_plain_error` for a fixed detail-free 400 and terminate. Never echo/log callback query data, fetch discovery, restart authentication, inspect/refresh credentials, apply auth delay/challenge or clear cookies. Preserve valid callbacks and all non-callback behavior.
- Add only network-free `MockPluginSession` coverage. No integration edit or new integration issue; D0058 remains separate.

## Approved implementation plan

- Fresh child implementer `b0297_implementation` proposed checking callback `code` and `state` once and handling only two nonempty values; all other callback forms terminate with a fixed 400 before ordinary authentication.
- Fresh child reviewer `b0297_plan_review` first returned Must Fix because `ErrTerminateRequest` needs an access-log diagnostic. The implementer revised the plan to emit only static `plugin_log.push_err("invalid callback")` before the fixed response.
- Re-review approved. Network-free restrictive mocks must cover provider error and empty/missing inputs, exact response/EOS/shutdown/terminal state, static diagnostic presence, and absence of all attacker query values from response/serialized PluginLog. Exact two-file scope remains frozen.

## Implementation

- The same child implementer changed only `request_filter_impl.rs` and `tests.rs`, with no plan deviation. Invalid callback forms now record only static `invalid callback`, write fixed `400 Invalid OIDC callback`, and terminate before authentication fallback.
- A restrictive table-driven mock test covers provider error without required parameters, empty code, empty state and missing state; it asserts exact response/no Location/EOS/one shutdown/terminal, static serialized diagnostic and absence of attacker/query values. Discovery/auth/cookie paths are not available to the mock.
- Exact network-free test passed 1/1 (4,386 filtered out); scoped rustfmt and diff checks passed.
- Fresh child final reviewer `b0297_final_review`: LGTM, no MustFix/ShouldFix; D0058 remains the only deferred item. It independently verified valid/non-callback preservation, early invalid-callback termination, fixed detail-free response/diagnostic, SessionAdapter ownership, restrictive mock value and exact two-file containment.
- Root format, locked workspace check (12.07s), locked Clippy (27.40s; existing warnings, exit 0), agent-doc, SSA, metrics, Pingora, Gateway tracing, English, frozen-integration and diff gates passed.
- Ledger/changelog updated. Commit pending automatic local commit; no integration edit or push.
