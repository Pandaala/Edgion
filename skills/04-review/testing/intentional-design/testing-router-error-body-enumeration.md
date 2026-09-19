---
name: testing-router-error-body-enumeration
description: Use when reviewing Gateway integration-testing API findings that client not-found bodies disclose the same client-name inventory as dedicated testing endpoints, or that create_testing_router lacks a local auth layer; the testing routes inherit the merged Admin router's bearer middleware.
knowledge_type: intentional-design
---

# Testing-router error-body enumeration behind inherited Admin auth — ACCEPTED-TRADEOFF

## Conclusion

The `Available: {:?}` client-name enumeration in
`get_redis_client_by_name` / `get_etcd_client_by_name` / `get_es_client_by_name`
not-found error bodies in `edgion-gateway/src/api/mod.rs` are **accepted under
the integration-testing-mode boundary model** and not patched in isolation.
`create_testing_router` intentionally has no local auth layer:
`create_admin_router` merges it first, then applies the shared bearer middleware
to the fully merged router.

Fix suggestions of the form "strip `Available: {:?}` from the three helper error bodies
to avoid topology disclosure" or "add a second token middleware directly to
`create_testing_router`" are **not accepted** as F-004-scoped fixes.

## Core Rationale

**1. The testing router intentionally publishes the same enumeration as a first-class endpoint**

The same router already exposes three dedicated client-listing endpoints behind
the inherited Admin bearer boundary:

- `GET /api/v1/testing/link-sys/redis/clients` → `redis_list_clients` returns
  `Vec<String>` of every configured Redis client name.
- `GET /api/v1/testing/link-sys/etcd/clients` → `etcd_list_clients` returns the
  same for Etcd.
- `GET /api/v1/testing/link-sys/es/clients` → `es_list_clients` returns the same
  for Elasticsearch.

These endpoints exist because integration tests need to assert "which clients did the
Controller sync to this Gateway". Stripping `Available: {:?}` from the not-found error
bodies would not reduce the disclosed surface — every caller that can hit the
`{name}/...` routes can also hit `/clients` and read the full list directly. The
not-found-body enumeration is a debug-ergonomics dup of an intentional feature, not a
new information channel.

**2. Mounting is gated by an explicit operator opt-in with a production warning**

`create_admin_router` only merges `create_testing_router` when
`is_integration_testing_mode()` is true. The
`edgion-common/src/test_mode.rs::init_integration_testing_mode` setter emits a
`tracing::warn!` on enable that reads "Integration testing mode enabled … **DO NOT
enable in production**". A production operator turning the flag on is committing to
running test fixtures, and the entire `/api/v1/testing/...` surface — access-log store
reads, store-stats, client enumeration, client mutations — is a single fixture surface,
not a security boundary.

**3. `create_testing_router` inherits the router-wide Admin authentication boundary**

`create_admin_router` first constructs the ordinary Admin routes, conditionally
merges `create_testing_router`, and applies `admin_auth_middleware` last. The
resulting single middleware covers both ordinary and testing routes. Adding a
second local layer to `create_testing_router` would duplicate the mechanism and
risk divergent response semantics without adding protection.

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
- Add another token middleware directly to `create_testing_router` — the merged
  router already applies the canonical middleware, so this would create a
  divergent duplicate boundary.
- Switch the URL underscore-encoding (`name.replacen('_', "/", 1)`) to a different
  delimiter — Kubernetes namespaces forbid `_` (RFC 1123), so the current decoder is
  unambiguous; this was the original Round A concern, disproven in the issue write-up.
- Move client enumeration onto the ordinary Admin router — both routers already
  share the bearer boundary, while enumeration remains a test-fixture feature
  rather than operator tooling.

## Re-evaluation Triggers

Re-open this decision only if:

- `create_admin_router` stops applying its middleware after merging the testing
  router, leaving testing routes outside the canonical boundary.
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
- `skills/04-review/admin-api/regression-guards/gateway-admin-bearer-boundary.md`
  — the current router-wide authentication contract.
- `edgion-gateway/src/api/mod.rs` — `create_admin_router`,
  `create_testing_router`, `get_redis_client_by_name`,
  `get_etcd_client_by_name`, `get_es_client_by_name`, and the
  `redis_list_clients` / `etcd_list_clients` / `es_list_clients` route entries.
- `edgion-common/src/test_mode.rs` — `init_integration_testing_mode`
  production-warning emission and `is_integration_testing_mode` reader.

## Resolution references

- `infra-cluster-F004` — 2026-05-27 (⚠️ accepted tradeoff).
