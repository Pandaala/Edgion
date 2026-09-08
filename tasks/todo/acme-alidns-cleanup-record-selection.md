# Select AliDNS Cleanup Records Exactly

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |
| Source | AliDNS cleanup response-validation follow-up |

## AI guide summary

Make AliDNS DNS-01 cleanup delete only the TXT record created for the current challenge by replacing fuzzy, unbounded lookup handling with exact RR/type/value ownership checks and bounded pagination.

## Requirements

- Use the `value` supplied to `remove_txt_record`; do not ignore the challenge value or treat every record returned by an RR keyword query as owned by the current cleanup operation.
- Match each candidate against the exact expected RR, record type `TXT`, and exact challenge value before using its `RecordId` for deletion.
- Do not rely on `RRKeyWord` fuzzy matching as proof of ownership. Either use an exact provider filter with verified semantics or apply exact local validation to every returned record.
- Traverse all relevant AliDNS result pages within the existing cleanup-operation deadline. Use checked pagination arithmetic, finite page and record bounds, and explicit repeated/no-progress detection so malformed metadata cannot cause an unbounded loop or silently incomplete traversal.
- Preserve signed request construction, the shared outbound client, redirects policy, connect/request deadlines, bounded response reading, HTTP/JSON/business-response validation, and normalized error classification.
- Preserve serial per-record best-effort deletion: attempt later exactly matched records after one delete fails, retain only the first normalized error, return it after the loop, and let the Controller service emit its existing non-fatal warning before continuing with later domain/value cleanup entries.
- Do not expose record IDs, TXT values, signed URLs, credentials, provider `Code` or `Message`, response bodies, pagination payloads, or dependency error chains through errors, status, or logs.
- Do not add provider-local logs, parallel observability, new metrics, retries, tasks, or a new failure-policy shape.
- Add deterministic focused unit tests without network access. Cover exact and near-match RR values, non-TXT records, matching and nonmatching challenge values, mixed pages, multiple pages, empty completion, repeated/no-progress pages, malformed or overflowing pagination metadata, hard page/record limits, per-record continuation, first-error retention, and redacted diagnostics.
- Do not modify existing integration-test files; record a separate gap only if focused unit tests cannot prove a required cross-process behavior.

## Scope

### In scope

- AliDNS TXT-record lookup, exact ownership filtering, pagination, and deletion selection in `edgion-controller/src/services/acme/dns_provider.rs`.
- Focused in-crate test support needed to inject deterministic paginated responses.

### Out of scope

- Changes to AliDNS signing, credentials, provider endpoints, challenge creation, propagation polling, issuance scheduling, or public configuration.
- Cloudflare, Pebble challtestsrv, and other DNS-provider behavior.
- Changes to the existing Controller-owned non-fatal cleanup policy, logs, metrics, or integration suites.

## Acceptance criteria

- Cleanup deletes a returned record only when its RR, type, and value exactly identify the current DNS-01 challenge; near matches and unrelated TXT records are never deleted.
- Every relevant page is processed exactly once within checked hard bounds and the existing operation deadline; repeated, non-progressing, malformed, overflowing, or excessive pagination fails safely with a bounded classification.
- A failed deletion does not prevent later exactly matched deletions, and only the first normalized failure reaches the existing service warning boundary.
- Diagnostics contain none of the sensitive or provider-controlled values excluded above, including through alternate `anyhow` display and source chains.
- Existing signing, outbound transport, byte caps, response validation, ordering, and service continuation behavior remain unchanged.
- Deterministic focused unit tests cover ownership, pagination, bounds, continuation, and redaction; existing integration tests remain unchanged.
- Controller format, targeted tests, workspace check/Clippy, documentation validation, and repository static guards pass.
