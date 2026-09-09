# B0199 — TCP Proxy

## Scope

- Pipeline state: terminal.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tcp/edgion_tcp.rs`.
- Baseline: `21ca0728`.
- Freshness confirmed at `3e3e3606`, including B0198 schema sharing; the selected route-unit Arc still owns its immutable generation. Fresh TCP baseline 18/18.
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
- D0112 tracks timeout pre-resolution; D0113 tracks stale connect-timeout knowledge. Hooks tests use ordinary in-memory duplex streams and direct error callbacks without modifying shared test helpers.

## Implementation and Review

- Implementer: `b0199_implementation`; exact three-file technical scope respected.
- A synchronous production selection helper releases the table guard before awaits while retaining the selected unit generation. Upstream address ownership moves into context and is borrowed for peer creation without changing diagnostics or validation order.
- Four new tests exercise production selection Weak ownership, old/new generations, actual relay accounting, paused idle/backpressured writes and every error mapping.
- Reviewer: `b0199_independent_review`; LGTM, no Must Fix or Should Fix. Verified production ownership seam, actual BasicPeer address ownership, every security gate and knowledge limits; independently reran TCP 22/22. Five target Clippy manual-let-else warnings were verified as existing HEAD structures.

## Verification

- Final TCP tests 22/22; L4 relay 21/21; TCP logs 4/4.
- Workspace format, agent-doc (1,259 files), SSA, metrics, Pingora, Gateway tracing, English-only, diff and frozen-path checks passed.
- Locked workspace check/Clippy passed, all route tests 585/585, Gateway binary build passed.

## Handoff

- Ledger/changelog updated: yes.
- Commit: `b833f400`.
