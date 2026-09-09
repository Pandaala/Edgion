# B0199 — TCP Proxy

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tcp/edgion_tcp.rs`.
- Baseline: `21ca0728`.
- Analyst: `b0199_tcp_proxy_analysis`.
- Investigate accept/connect/relay lifecycle, IP trust and egress boundaries,
  plugin dispatch, timeout/health/accounting, and canonical observability.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Decision: change; simple. Target blob: `84d2e71afb4ce8358328d06f2f24539222ec7aba`; fresh baseline tests 3/3 at `21ca0728`.
- `match_route` returns an owned route-unit Arc, but the current route-table guard survives through plugins/connect/relay. Limit load/match scope so a long connection retains only its selected unit, not the old table and unrelated candidates.
- Move the upstream address String into context and borrow it for peer construction instead of cloning; preserve unsafe-address diagnostics and dial/validation ordering.
- Proposed exact scope: target; new `skills/04-review/routing/regression-guards/tcp-connection-snapshot-ownership.md`; routing index. Use a production selection helper only if it earns its scope; verify old-table release/selected-unit survival with Weak and actual production selection, not duplicated test-only expressions.
- Verify TCP hooks via in-memory relay and run TCP, L4 relay and TCP-log filters. Existing transport integration remains frozen; any new scenario is deferred.
- Defer cross-L4 connection-timeout pre-resolution to one shared listener-runtime design; no TCP-only mechanism. Direct typed peer construction, log serialization clones and sibling schema work are outside this batch.
- Security/context review retained DirectPeerIp vs RemoteIp, stream fail-close, ReferenceGrant/kind/endpoint checks, default connect/idle bounds, token-scoped outlier accounting, canonical masking and B0196 relay behavior.
- Connect-timeout knowledge has stale FailureClass/CRD history; do not present it as current code evidence.
