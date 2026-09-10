# B0292 — HTTP OIDC Auth Response Caps Integration Tests

- Pipeline state: changed; implementation/final LGTM and all checks passed, local commit gate.
- Active anchor: `edgion-gateway`; promoted baseline `3a13d625`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/auth_response_caps_integration.rs`, a 272-line cfg(test)-only module with eight ignored loopback-network regressions.
- Analyst: `b0292_oidc_caps_analysis`; full target, production clients/caps/egress, deterministic response tests and D0036 knowledge examined.
- Concrete change: replace the stale line-7 reference to deleted `tasks/todo/oidc-auth-response-byte-caps.md` with the durable `skills/04-review/oidc/regression-guards/authentication-response-byte-caps.md` reference. Exact scope is target only; no production behavior or test logic change.
- Preserve all eight ignored regressions: they uniquely cover real endpoint wiring/framing, Auto JWT fallback, status/static diagnostics and early Content-Length rejection. Deterministic sibling tests already own stream/parser mechanics; do not add a transport seam for mock convenience.
- Verification at promotion: scoped rustfmt check and scoped diff check only. Do not run the ignored tests because they bind loopback sockets and perform network IO. No new deferred task; existing D0036 knowledge owns the boundary.
- B0291 changed an unrelated Guardrail file; target remains unchanged. Fresh implementer may change only the stale source pointer and must not modify test logic or run network tests.

## Implementation, review and verification

- Changed only line 7's source pointer, retaining D0036. Exact scoped diff is one deletion/one addition; hashes excluding that line prove all test logic/helpers/macros unchanged.
- Fresh final reviewer `b0292_final_review`: LGTM, no MustFix/ShouldFix/deferred finding. The old task is absent; replacement knowledge exists, is indexed and accurately documents the boundary.
- Scoped rustfmt/diff checks and fresh root format, locked workspace check (10.64s), locked Clippy (24.35s, existing warnings, exit 0), agent-doc, SSA, metrics, Pingora, tracing, English, freeze and diff gates passed.
- The eight ignored loopback-network tests were intentionally not run. No integration edit. Commit pending; no push.
