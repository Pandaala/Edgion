# Fail closed on invalid Gateway BackendTLSPolicy candidates

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

Wire the canonical BackendTLSPolicy identity validator into Gateway publication without making an invalid TLS policy indistinguishable from an absent policy. The store currently trusts the Controller's resolved marker, while a hand-crafted FileSystem, etcd, or conf_sync payload can carry that marker with a malformed hostname or typed SAN. Simply omitting the policy from the effective index is unsafe because HTTP and gRPC callers interpret `None` as “no BackendTLSPolicy” and may construct a plaintext peer.

## Requirements

- Run `backend_tls_identity_validation_errors` before an eligible policy becomes effective in the Gateway.
- Represent lookup outcomes distinctly as absent, effective policy, and invalid/unresolved applicable policy.
- Fail the selected backend request closed when an applicable policy is invalid or cannot be resolved; never silently downgrade to a peer without TLS.
- Scope invalidity by typed target and Service section so a policy for an unrelated section cannot block traffic.
- Let a valid policy win according to the normal exact-section/whole-target and precedence rules when invalid candidates are skipped by that arbitration contract.
- Preserve generic client errors and bounded access-log diagnostics without logging hostnames, SAN identities, CA material, client certificates, or full specs.
- Apply identical semantics to HTTP and gRPC Service paths and AI EdgionBackend plan capture.

## Scope

### In scope

- BackendTLSPolicy publication eligibility and lookup result type.
- HTTP, gRPC, and AI callers that capture or consume the policy.
- Peer-construction failure mapping and regression tests for plaintext downgrade.

### Out of scope

- BackendTLSPolicy API-version migration.
- Certificate parsing or SAN matching implementation.
- Gateway rustls backend support or parity.

## Acceptance criteria

- Malformed hostname and malformed/oversized/type-inconsistent SAN inputs never enter an effective policy.
- An applicable invalid-only policy causes a deterministic fail-close result before any plaintext connection attempt.
- An invalid policy for another Service section does not block the selected section.
- A valid applicable winner remains usable when the arbitration rules permit invalid candidates to be skipped.
- Missing Service data or an unresolved port-to-section mapping cannot turn a present section-scoped TLS policy into `None` and plaintext fallback.
- Full and partial publication produce the same absent/effective/invalid lookup outcome.
- HTTP, gRPC, and AI tests prove no policy-error path constructs a plaintext peer.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/07-backends.md`
- `skills/02-features/03-resources/12-backend-tls-policy.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Do not land publication-only filtering. | The current `Option<Arc<BackendTLSPolicy>>` API collapses invalid and absent states; filtering alone can convert a policy error into a plaintext downgrade. |
