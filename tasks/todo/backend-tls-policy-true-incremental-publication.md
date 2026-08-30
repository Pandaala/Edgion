# Make BackendTLSPolicy publication truly incremental

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-31 |
| Status | pending |
| Type | refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Implement a real incremental update path for the Gateway `BackendTLSPolicyStore` while retaining one immutable, atomically published policy/index generation. The current `partial_update` merges deltas into a clone of the complete policy map and then rebuilds the complete reverse index, violating the `ConfHandler` incremental-update contract and making small hot-reload events cost O(total policies and attachments).

## Requirements

- Apply add, update, and remove events without rebuilding unaffected target buckets.
- Recompute the union of targets named by each changed policy's old and new generations so moved or removed attachments cannot remain stale.
- Publish the policy map and reverse index atomically in one immutable snapshot generation.
- Preserve exact-section-first and whole-Service fallback behavior; an unrelated section must never win.
- Preserve oldest-creation-timestamp then name precedence and typed Service versus EdgionBackend isolation.
- Retain equivalent policy `Arc`s and suppress generation changes for semantically equivalent batches.
- Keep the configuration writer serial and request-path reads lock-free.

## Scope

### In scope

- `edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs`.
- `BackendTLSPolicyStore` full and partial publication algorithms.
- Focused equivalence, moved-target, removed-target, multi-policy conflict, and generation tests.

### Out of scope

- BackendTLSPolicy schema or Controller arbitration changes.
- Backend TLS peer construction and certificate parsing.
- Gateway rustls backend support or parity.

## Acceptance criteria

- A one-policy update does not clone or rescan unrelated policy or reverse-index entries.
- Old-only and new-only affected targets are both recomputed in the same publication.
- A batch touching multiple targets yields exactly one snapshot generation change.
- Readers can never observe a policy map and reverse index from different generations.
- Equivalent updates retain `Arc` identity and do not advance the generation.
- Full-set replacement, empty-set clearing, and partial add/update/remove produce equivalent effective snapshots.
- Exact-section, whole-target fallback, typed-target isolation, missing-marker rejection, and oldest-wins tests remain green.
- Workspace check, Clippy, and repository guards pass.

## Related guidance

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/07-backends.md`
- `skills/01-architecture/02-gateway/11-conf-handler-guidelines.md`
- `skills/02-features/03-resources/12-backend-tls-policy.md`
- `skills/05-testing/SKILL.md`

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-31 | Defer the incremental refactor from the file audit. | A safe fix must update old and new target buckets while publishing the map and index atomically; a local partial mutation could expose stale winners or split generations. |
