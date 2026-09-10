# Assess Authentication Credential Ownership Follow-Ups

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | investigation |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Evaluate the optional credential copy and retention questions listed below one ownership boundary at a time, promoting only changes with a concrete security or memory-residency benefit that preserve correctness and hot-path performance.

## Requirements

- Start from `skills/04-review/security/review-evidence/credential-copy-ownership.md` and revalidate each candidate against current source before proposing a change.
- Assess separately the recorded parser/seen-map duplicates, HMAC runtime-map ownership, JWT per-issuer and request extraction copies, managed-credential temporary/header ownership, shared TLS material projection, and optional ACME EAB/DNS construction copies.
- For every candidate, identify the exact owner, why the copy exists, its release event, outstanding shared or third-party owners, exposure reduction, performance cost, and correctness constraints.
- Record an explicit keep/change/defer decision for each ownership boundary. Do not infer value from allocation count alone and do not combine unrelated owners into a blanket migration.
- Use `Zeroizing` only for a uniquely owned buffer with a meaningful last-owner boundary. Never wipe shared config/cache generations in place or claim allocator, SDK, crypto-schedule, transport-buffer, persisted Secret, or log-history erasure.
- Preserve authentication behavior, failure policy, hot reload, retry/singleflight semantics, redaction, binary-owned logging, and canonical access-log ownership.
- Split any approved implementation into narrowly scoped follow-up work with its own tests; do not modify existing integration tests as part of this investigation.

## Scope

### Selected narrow implementation

- KeyAuth parser/seen-map boundary: change approved. Remove the redundant raw-key
  seen map and move parsed key/allowed metadata strings into the result map.
  Scope: Controller `resolve_credential/key_auth.rs` and its unit tests only;
  no shared schema, Gateway runtime, or integration-test changes.
  Evidence and verification live in the existing
  [ownership reference](../../skills/04-review/security/review-evidence/credential-copy-ownership.md#keyauth-parser-ownership-follow-up).
  Six focused tests passed and independent read-only review returned LGTM.
  Format, workspace check/clippy, SSA-force, metrics inventory, Pingora source
  policy, and whitespace checks passed. Agent-doc validation remains blocked
  by existing links to the missing `tasks/todo/remove-tlsroute-v1alpha3-fallback.md`;
  closure is withheld. See the ownership reference for verification limits.
- Remaining candidates stay pending: BasicAuth and HMAC parser duplicates,
  HMAC runtime-map ownership, JWT copies, managed-credential ownership, shared
  TLS projection, and ACME EAB construction. DNS-01 was removed; its historical
  candidate does not authorize restoring DNS functionality.

### In scope

- Optional residual copy/retention questions listed in this task and the maintained credential-ownership reference.
- Source-backed ownership and value analysis, with narrow implementation follow-ups only when justified.

### Out of scope

- A universal credential destruction deadline or repository-wide zeroization claim.
- Changes outside the listed copy/retention boundaries, including reopening completed ownership fixes or unrelated authentication defects.
- Third-party allocator, SDK, operating-system, remote-service, and transport erasure guarantees.

## Acceptance criteria

- Every recorded candidate has an ownership-specific evidence table and keep/change/defer decision.
- Proposed changes state which concrete copy is removed, which independent copies remain, and why behavior and performance are preserved.
- Any implementation work is separately scoped and tested without weakening redaction or observability rules.
- The maintained credential-ownership evidence remains truthful: release events are not presented as physical erasure guarantees.
- Documentation validation and `git diff --check` pass; existing integration tests remain unchanged.
