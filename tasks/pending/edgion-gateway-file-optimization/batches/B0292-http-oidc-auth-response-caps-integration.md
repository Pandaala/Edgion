# B0292 — HTTP OIDC Auth Response Caps Integration Tests

- Pipeline state: analysis-ready; target-only documentation correction candidate, no mutation authorized while B0291 owns the serial lane.
- Active anchor: `edgion-gateway`; baseline `0241f37c`.
- Target: `edgion-gateway/src/plugins/http/openid_connect/auth_response_caps_integration.rs`, a 272-line cfg(test)-only module with eight ignored loopback-network regressions.
- Analyst: `b0292_oidc_caps_analysis`; full target, production clients/caps/egress, deterministic response tests and D0036 knowledge examined.
- Concrete change: replace the stale line-7 reference to deleted `tasks/todo/oidc-auth-response-byte-caps.md` with the durable `skills/04-review/oidc/regression-guards/authentication-response-byte-caps.md` reference. Exact scope is target only; no production behavior or test logic change.
- Preserve all eight ignored regressions: they uniquely cover real endpoint wiring/framing, Auto JWT fallback, status/static diagnostics and early Content-Length rejection. Deterministic sibling tests already own stream/parser mechanics; do not add a transport seam for mock convenience.
- Verification at promotion: scoped rustfmt check and scoped diff check only. Do not run the ignored tests because they bind loopback sockets and perform network IO. No new deferred task; existing D0036 knowledge owns the boundary.
