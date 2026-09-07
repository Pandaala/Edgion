---
name: testing-router-error-body-enumeration
description: Use when reviewing Gateway integration-testing API findings of the form "get_redis/etcd/es_client_by_name leaks full client list in not-found error body / create_testing_router has no auth middleware / testing endpoints disclose infrastructure topology"; covers why error-body enumeration and the auth gap on the testing router are accepted under the integration-testing-mode boundary model.
knowledge_type: intentional-design
---

# Testing-router error-body enumeration & no-auth on `create_testing_router` — ACCEPTED-TRADEOFF

## Conclusion

The `Available: {:?}` client-name enumeration in
`get_redis_client_by_name` / `get_etcd_client_by_name` / `get_es_client_by_name`
not-found error bodies (`edgion-gateway/src/api/mod.rs:679-687`, `:972-980`, `:1246-1254`),
and the absence of any authentication middleware on `create_testing_router`
(`edgion-gateway/src/api/mod.rs:531-666`), are **accepted under the integration-testing-mode
boundary model** and not patched in isolation.

Fix suggestions of the form "strip `Available: {:?}` from the three helper error bodies
to avoid topology disclosure" or "add a shared-secret / token middleware to
`create_testing_router`" are **not accepted** as F-004-scoped fixes.

## Core Rationale

**1. The testing router intentionally publishes the same enumeration as a first-class endpoint**

The same router already exposes three dedicated client-listing endpoints with no
authentication and no redaction:

- `GET /api/v1/testing/link-sys/redis/clients` → `redis_list_clients`
  (`api/mod.rs:583`) returns `Vec<String>` of every configured Redis client name.
- `GET /api/v1/testing/link-sys/etcd/clients` → `etcd_list_clients`
  (`api/mod.rs:619`) returns the same for Etcd.
- `GET /api/v1/testing/link-sys/es/clients` → `es_list_clients`
  (`api/mod.rs:629`) returns the same for Elasticsearch.

These endpoints exist because integration tests need to assert "which clients did the
Controller sync to this Gateway". Stripping `Available: {:?}` from the not-found error
bodies would not reduce the disclosed surface — every caller that can hit the
`{name}/...` routes can also hit `/clients` and read the full list directly. The
not-found-body enumeration is a debug-ergonomics dup of an intentional feature, not a
new information channel.

**2. Mounting is gated by an explicit operator opt-in with a production warning**

`create_admin_router` only merges `create_testing_router` when
`is_integration_testing_mode()` is true (`api/mod.rs:224`). The setter
`init_integration_testing_mode` (`edgion-common/src/test_mode.rs`) emits a
`tracing::warn!` on enable that reads "Integration testing mode enabled … **DO NOT
enable in production**". A production operator turning the flag on is committing to
running test fixtures, and the entire `/api/v1/testing/...` surface — access-log store
reads, store-stats, client enumeration, client mutations — is a single fixture surface,
not a security boundary.

**3. The auth gap on `create_testing_router` is a subset of the broader F-001 problem,
not an F-004-local fix**

`tasks/infra/infra-cluster-F001-api-no-auth-on-admin-endpoints.md` (High, open) tracks
the real boundary defect: `create_admin_router` mounts the **base** router (always
present, not gated by integration-testing mode) on `0.0.0.0:5900` with no auth and no
TLS, exposing `EdgionTls.spec.secret` (resolved TLS private keys) and `LinkSys`
resolved backend credentials. The correct fix is a router-wide auth/bind hardening at
the F-001 level, which subsumes the testing-router gap as a side effect. Adding a
secondary auth layer only to `create_testing_router` would still leave the private-key
and backend-credential disclosure on the base router unfixed and create two divergent
auth paths on the same `:5900` listener — a worse posture than the current single
boundary.

**4. Removing the enumeration would degrade integration-test debug-ergonomics for zero
real-world gain**

The `Available: {:?}` suffix is what lets a failing test surface "you asked for
`redis_main` but the sync produced `redis-main` / `redis_replica`" without an extra
round-trip to `/clients`. The cross-namespace confusion attack the original Round A
write-up worried about does not apply (Kubernetes namespaces follow RFC 1123 — letters,
digits, hyphens only, no underscores — so `replacen('_', "/", 1)` always produces the
correct `namespace/name` split for any admissible namespace), so the residual
"topology disclosure" framing is the only remaining concern, and §1 above already
neutralizes it.

## Fix Suggestions Not Accepted

- Strip `Available: {:?}` from the three not-found error bodies — no surface reduction
  (same data published by `/clients`), measurable test-debug regression.
- Add shared-secret / token middleware to `create_testing_router` only — leaves the
  base router's private-key disclosure unfixed; creates a divergent auth path on the
  same listener.
- Switch the URL underscore-encoding (`name.replacen('_', "/", 1)`) to a different
  delimiter — Kubernetes namespaces forbid `_` (RFC 1123), so the current decoder is
  unambiguous; this was the original Round A concern, disproven in the issue write-up.
- Move client enumeration off the testing router into the base router behind auth —
  exactly inverts the boundary model: enumeration is a test-fixture feature, not an
  operator-tooling feature; the base router is for `/configclient` resource reads,
  which is the F-001 surface.

## Re-evaluation Triggers

Re-open this decision only if:

- F-001 is fixed in a way that adds auth to the base router but leaves
  `create_testing_router` un-layered — at that point a small "extend the same auth to
  the testing router" follow-up is in scope.
- An integration-testing-mode boundary change makes the flag controller-side instead
  of process-startup-side (i.e. an attacker could flip the flag at runtime without
  control over the binary), invalidating the "operator opted in at process start" gate.
- A concrete deployment pattern emerges where the testing router is required in a
  production-adjacent network with no NetworkPolicy isolation, making the
  warned-against state the de-facto reality.
- A new testing endpoint is added that returns secret material (private keys, bearer
  tokens, HMAC secrets) — that endpoint must not ship without auth regardless of this
  rule.

## Reference Cases

- `tasks/infra/infra-cluster-F004-api-name-separator-ambiguity.md` (Round A) — closed
  2026-05-27 as ⚠️ accepted tradeoff.
- `tasks/infra/infra-cluster-F001-api-no-auth-on-admin-endpoints.md` (open, High) —
  the broader Gateway admin-API auth/bind issue that subsumes the testing-router auth
  gap.
- `edgion-gateway/src/api/mod.rs` — `create_admin_router` (`:212-229`),
  `create_testing_router` (`:531-666`), `get_redis_client_by_name` (`:672-687`),
  `get_etcd_client_by_name` (`:965-980`), `get_es_client_by_name` (`:1239-1254`),
  `redis_list_clients` / `etcd_list_clients` / `es_list_clients` route entries
  (`:583`, `:619`, `:629`).
- `edgion-common/src/test_mode.rs` — `init_integration_testing_mode`
  production-warning emission (`:42-53`), `is_integration_testing_mode` reader
  (`:57-59`).

## Resolution references

- `infra-cluster-F004` — 2026-05-27 (⚠️ accepted tradeoff).
