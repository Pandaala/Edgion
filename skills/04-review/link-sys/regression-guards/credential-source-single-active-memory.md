---
name: credential-source-single-active-memory
description: Use when reviewing CredentialSource memory publication, token rotation, or proposals to retain previous generated tokens.
knowledge_type: regression-guards
---

# CredentialSource memory publication retains one active token

## Conclusion and scope

For `publication.persist: false`, the Controller's `CredentialMemoryStore` holds
one `Arc<GeneratedSecretEntry>` per LinkSys identity (`namespace/name/uid`). A
successful publication atomically replaces the active entry under the existing
lock. The store has no previous-token lookup or fallback. `publication` exposes
only `persist`; the removed `memoryMaxKeys` field is rejected by strict serde
decoding and is absent from generated schema and serialized configuration.

This is a scoped simplification of optional retention, authorized on 2026-10-06
after examining rotation and recovery. It does not establish a prior correctness
or security defect, nor a physical-erasure guarantee. It preserves the independent
credential owners accepted in the
[credential ownership tradeoff](../../security/intentional-design/credential-copy-retention.md).

## Rationale and evidence

Previously, `insert_oauth_publication` rebuilt an `IndexMap` with the new active
token and, when the configured cap allowed it, the previous active token. OAuth
publication never retained more than two entries despite a default cap of 10000.
Only `active()` exposed credential material; neither Injector resolution nor
service cleanup selected the previous entry.

Current source and call-chain boundaries:

- `edgion-controller/src/services/credential_manager/memory.rs`: identity-keyed
  active entries; `value` remains `Zeroizing<Vec<u8>>`, with redacted Debug.
- `service.rs`: `try_publish_persist_off` retains worker/source/publication checks,
  then publishes without capacity errors. Acquisition and `RetainedPublication`
  remain separate from the active memory entry.
- `handlers/plugins/resolve_credential/credential_injector.rs`: `memory_material`
  obtains the active entry and validates its monotonic expiry before embedding
  token material in the consumer's `resolvedCredential`.
- `edgion-resources/src/resources/link_sys/credential_source.rs`: strict
  `CredentialPublicationConfig` contains only `persist`.

`credential_memory_keys` counts active store entries, one per stored source
identity. It excludes retired entries held by external Arcs, Gateway runtime
generations, and publication retry state. Counts obtained by separate calls are
not a coherent snapshot when publication or cleanup races the diagnostic read.

## Alternatives and limits

Proactive acquisition remains scheduled at the earlier of `rotation.interval`
and token half-life, with earlier-only jitter and a 30-second consumer expiry
margin. A failed acquisition does not replace the current token. Secret
publication retries use `RetainedPublication`, and publication-mode transitions
retain material until replacement publication completes. Bootstrap
`previousSecretRef` is used on typed `invalid_client`, not as generated-token
fallback. None of these mechanisms needs the unread previous store entry.

Replacing or clearing an entry drops only the store reference. Existing external
Arcs remain valid until their holders release them; the final strong owner drops
the zeroizing value. Already-published Gateway generations and request-pinned
headers have independent ownership and expiry checks. No Gateway/conf-sync owner
rewrite or automatic upstream-authentication fallback is part of this change.

## Verification and re-evaluation

Regression coverage checks repeated rotation, release of unread previous entries,
external Arc survival, namespace/name/UID isolation, active-only diagnostic counts,
Injector re-resolution, expiry, cleanup, publication fences, and mode transitions.
Fresh targeted regressions passed on 2026-10-06:

- `cargo test -p edgion-resources --lib credential_source`: 8 passed.
- `cargo test -p edgion-controller --lib services::credential_manager`: 43 passed.
- `cargo test -p edgion-controller --lib resolve_credential::credential_injector`:
  13 passed.
- `cargo test -p edgion-controller --lib handlers::link_sys::tests`: 33 passed.
- `cargo test -p edgion-controller --lib api::debug_handlers::tests`: 5 passed.

An independent read-only Astra review approved the implementation, scope, and
knowledge classification. The CRD/condition coverage script passed 27 tests.
`cargo fmt --all`, workspace all-target check and Clippy, SSA-force, metrics
inventory, Pingora-source policy, Gateway API transform contract, unit-test layout,
and `git diff --check` passed. The unrelated WAF task-link blocker was resolved,
and agent-document validation passed again at closure on 2026-10-06. The user's
shared-topology integration report
(`tmp/integration_testing/testing_20261006_121044/report.log`) records 257 passed,
seven skipped, zero failed and zero timed out, including CredentialInjector and
its dynamic scenario. No full workspace unit-test run is claimed.

Re-evaluate if a concrete consumer requires an authorized previous-generation
lookup or recovery policy. Design that policy and its expiry, publication, and
issuer-validity semantics explicitly rather than restoring unused retention.

## Resolution references

- `simplify-credential-source-memory-retention`: optional P3 refactor; the user
  authorized single-active storage and removal of the capacity field on 2026-10-06.
  The user authorized closure and deletion of the task after verification.
- [Credential lifecycle](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/credential-injector.md#credential-lifecycle).


## Actual consumer/source and local material boundary

Desired consumer output coordinates now derive from its frozen actual Injector
field rather than independent strings. Pair validation binds that consumer to the
selected captured LinkSys source, including UID/generation, while retaining input,
Grant, live owner and publication checks. Admission, recovery adoption, fan-out,
completion, desired-publication checks, counts and memory clearing use the pair.
A pair mismatch must not mark another source's token as successfully published.

Qualified local generated Secret reads validate raw name/namespace before bearer
bytes are parsed; no TLS bare-default selection is added. Existing expiry, active
entry replacement and independent retained publication owners stay unchanged.
Cleanup expectations remain recoverable independently of token adoption and do
not require a live old source/consumer. API-returned output identity and orphan
cleanup IO are explicitly deferred to the next bounded output-IO batch; this
change does not strengthen their existing POST/409/CAS/delete protocol by itself.
