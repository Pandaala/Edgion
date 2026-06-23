---
name: cross-gateway-sni-collision-deterministic
description: All three TLS/SNI matchers (Gateway listener cert, EdgionTls cert, TLSRoute) now sort colliding slots by Gateway API precedence (oldest creationTimestamp, then namespace/name) with a config-sync warning, so selection is deterministic. Reject re-reports claiming match_sni/match_route non-deterministically picks the first of many entries.
---

# tls-F007 — TLS/SNI matchers select deterministically on same-hostname collisions

**Status:** ✅ fixed · 2026-05-20
**Severity:** High (BadLogic) — non-deterministic TLS certificate/route selection across rebuilds

## Conclusion

When two resources claim the same hostname on the same physical listener — a
collision the controller's intra-gateway-only conflict detection does not block —
the matcher slot holds several entries and the selection used to follow upstream
HashMap iteration order (it could flip on every rebuild/restart, silently). All
three affected matchers now sort each colliding slot at **rebuild time** by the
Gateway API conflict-resolution rule and emit a config-sync `tracing::warn!`. The
data-plane lookups (`match_sni` / `match_route`) are unchanged — they still return
`first()`, which is now a **deterministic** winner.

**Tiebreak = Gateway API precedence: oldest `creationTimestamp` wins, then namespace,
then name** (then `listener_name` for the Gateway-listener case). This matches the
house convention already used by `BackendTLSPolicyStore::sort_policies_by_precedence`,
and means the **incumbent** resource keeps serving when a newer one is added with a
colliding hostname (less surprising than lexicographic). `Option<Time>` orders `None`
first, so a missing timestamp is treated as oldest.

Three matchers, all fixed the same way:

1. **`GatewayTlsMatcher`** (`edgion-gateway/src/runtime/matching/tls.rs`) — Gateway
   listener certs. `rebuild_from_gateways` sorts colliding slots via `tls_entry_order`
   and warns via `warn_tls_sni_collision`. Added a `gateway_creation_ts: Option<Time>`
   field to `GatewayTlsEntry` (populated from `gateway.metadata.creation_timestamp`).
   Covers exact-hostname, **wildcard-hostname**, and catch-all slots.
2. **`TlsCertMatcher` / `TlsStore`** (`edgion-gateway/src/tls/store/tls_store.rs`) —
   EdgionTls certs. `rebuild_matcher_from_data` sorts each `Vec<Arc<EdgionTls>>` slot
   with `len > 1` via `edgion_tls_precedence` and warns via `warn_edgion_tls_collision`.
   `cert_matcher.rs::match_sni` unchanged.
3. **`TlsRouteTable`** (`edgion-gateway/src/routes/tls/gateway_tls_routes.rs`) — TLSRoute
   passthrough routing. `from_routes` sorts each colliding hostname bucket (and the
   catch-all) via `tls_route_precedence` and warns via `warn_tls_route_collision`.
   `match_route` unchanged. (Lower severity — picks a route/backend, not a cert — but
   the same non-determinism class, and the Gateway API tiebreak now applies.)

Re-reports of the form "`match_sni`/`match_route` silently returns only the first of
many entries", "same-hostname collision picks a non-deterministic cert/route", or
"winner flips across rebuilds with HashMap order" are **not accepted** for any of the
three matchers — verify the current rebuild path sorts colliding slots before
re-reporting.

## What was wrong (before the fix)

Each matcher stored `Vec<entry>` per (listener, hostname) slot and returned
`entries.first()`. Slots were filled by iterating a `HashMap` (Gateway slice from
`ConfHandler`, EdgionTls `tls_data`, TLSRoute `routes`), so insertion order — and thus
the winner — was non-deterministic and flipped across rebuilds. No log alerted the
operator. The wildcard sub-case in `GatewayTlsMatcher` was even worse: `get_mut_exact`
rejected the raw `"*.example.com"` form (the `"*"` label fails `is_valid_domain`) and
returned `None`, so the second Gateway fell into the `insert` arm and **silently
overwrote** the first (no `Vec` growth, no warning at all).

The controller's `detect_conflicts`
(`listener_port_manager.rs`) is **intra-gateway only** by Gateway API design (different
Gateways may independently use the same port), and there is **no** cross-resource
conflict detection for two EdgionTls or two TLSRoutes overlapping a host. So all three
collisions reach the data plane unguarded.

## Why the fix is correct

- Gateway API leaves cross-Gateway socket sharing undefined; Edgion merges resources
  onto one physical listener, so Edgion defines the tiebreak. Using the Gateway API
  route-conflict rule (oldest `creationTimestamp`, then ns/name) keeps it consistent
  with `BackendTLSPolicyStore` and gives operators the least-surprising "incumbent
  keeps serving" semantic. `creation_timestamp` is already on the kube objects the
  rebuild paths iterate, so plumbing it is one field (Gateway case) or zero (EdgionTls
  / TLSRoute read `.metadata` directly).
- Sorting happens at **rebuild time** (config-sync path), so the handshake/lookup paths
  stay plain `first()` calls with zero added cost.
- Every `tracing::warn!` lives only in a rebuild path, never in `match_sni`/`match_route`,
  so the data-plane handshake hot-path tracing prohibition
  ([03-coding/01-log-safety.md](../../03-coding/01-log-safety.md)) is respected.
- `HashHost::get_mut_exact` wildcard normalization (the GatewayTlsMatcher companion fix)
  has exactly one caller, so the behavior change is contained; it still refuses to fall
  back to a wildcard for a concrete subdomain.

## Why not the other directions

- "Reject the duplicate at insert time (keep Vec length ≤ 1)" → still
  HashMap-iteration-order dependent (first-inserted wins), so it does **not** fix
  determinism. Sorting does.
- "Warn only, keep first-seen order" → leaves the core correctness defect
  (non-deterministic winner) unfixed. Rejected during triage.
- "Use lexicographic (namespace, name) instead of creationTimestamp" → was the first
  cut, but it diverges from the codebase's own precedence convention and surprises
  operators (a newly-added `aaa-gw` would steal SNI from an older `zzz-gw`). Switched
  to creationTimestamp-first to match `BackendTLSPolicyStore`.

## Known limitations (not fixed here)

- **No cert-failover on collision.** Selection picks `entries.first()` and the losing
  entries are discarded; the consumption side (`apply_gateway_tls_cert`) fails closed
  if the winner's cert is unusable rather than falling through to a runner-up. This is a
  same-hostname misconfiguration path, and fail-closed is the safe behavior, but the
  deterministic winner is not "the entry that happens to have a valid cert".
- **Status-condition feedback is out of scope.** Surfacing the collision as a
  Gateway/Listener `Conflicted`/`Overlapping` status condition is architecturally
  infeasible from the data plane (the `ConfigSync` gRPC channel is pull-only — no
  reverse status RPC). The right home is the controller (where `ListenerPortManager`
  lives); tracked separately, not part of this fix.
- **Warn repeats per rebuild.** A standing misconfiguration re-warns on every config
  sync (and N−1 times per N-way slot). Acceptable for a misconfig that must be fixed;
  consider once-per-slot rate-limiting if it becomes noisy.
- **The "oldest wins" semantic depends on `creationTimestamp` reaching the data plane.**
  Verified (multi-agent trace, 2026-05-20): in **K8s mode** the field survives the full
  Controller→Gateway sync — the gRPC `ConfigSync` proto carries the whole object as a JSON
  `string data` payload, `clean_metadata` strips only blocked annotations + `managed_fields`,
  and serde round-trips `ObjectMeta` losslessly, so oldest-wins genuinely happens. In
  **FileSystem mode** (integration-test harness, `edgion-tests/integration/conf/...`), the field is
  present only if the source YAML includes `metadata.creationTimestamp`, which test fixtures
  usually omit. When `creationTimestamp` is **universally absent**, every entry ties on the
  timestamp and selection reduces to `(namespace, name)` lexicographic — i.e. the very
  behavior the "Why not the other directions" section rejected (a new `aaa-gw` would
  out-rank an older `zzz-gw`). Determinism is preserved either way; only the
  incumbent-keeps-serving guarantee is lost in the all-None case. To exercise timestamp
  ordering in FileSystem-mode tests, add explicit `creationTimestamp` to the fixtures
  (data change, not code).

## Regression guard

Unit tests in `tls.rs` (GatewayTlsMatcher):
- `test_cross_gateway_sni_collision_is_deterministic` / `_wildcard_collision_` /
  `_catchall_collision_` — same two Gateways rebuilt in both input orders pick the same
  winner (exact / wildcard / catch-all slots).
- `test_cross_gateway_collision_prefers_oldest_creation_timestamp` — the older Gateway
  wins even with a lexically larger name, proving creationTimestamp dominates.

Unit test in `hash_host.rs`:
- `test_get_mut_exact_resolves_wildcard_pattern` — `get_mut_exact("*.example.com")`
  resolves to its own bucket, still refuses a concrete subdomain, case-insensitive.

Unit tests in `tls_store.rs` (EdgionTls):
- `test_edgion_tls_precedence_prefers_oldest_over_name` /
  `_name_tiebreak_when_no_timestamp`.

Unit test in `gateway_tls_routes.rs` (TLSRoute):
- `test_same_hostname_collision_prefers_oldest_deterministically`.

Integration scenario `EdgionTls/sni_collision` (`edgion-tests/integration/conf/EdgionTls/sni_collision/`
+ suite `edgion-tests/src/client/suites/edgion_tls/sni_collision/`): two EdgionTls bind the same
host `shared-collision.example.com` on one HTTPS listener — older (`creationTimestamp`
2020-01-01) → RSA `edge-tls` (CN=test.example.com), newer (2024-01-01) → ECDSA
`edge-tls-ecdsa` (CN=ecdsa-dualcert.example.com). The test does an `openssl s_client`
handshake with that SNI and asserts the served subject CN is `test.example.com` (older), NOT
`ecdsa-dualcert.example.com`. This is the end-to-end proof that `creationTimestamp` survives
config-sync (FileSystem mode honors it because the fixtures set it explicitly) and the
`edgion_tls_precedence` sort picks the older cert on the wire. Like the `gateway_tls` dual-cert
test, it **skips when only LibreSSL is present** (macOS dev) and runs under genuine OpenSSL (CI).

## Re-evaluation triggers

- Gateway API defines explicit cross-Gateway / cross-resource SNI collision semantics —
  re-check the tiebreak against the spec.
- A matcher stops using `first()` (e.g. multi-cert SNI selection lands) — re-check the
  deterministic-slot invariant.
- A controller-side collision status condition is implemented — cross-link it here.

## Reference

- Issue: `tasks/issues/runtime-F007-gateway-tls-matcher-returns-first-entry-only.md` (Round B: confirmed, bump to High)
- Sibling matchers found by deep multi-agent review (TlsCertMatcher, TlsRouteTable) and fixed in the same pass.
- Convention reused: `BackendTLSPolicyStore::sort_policies_by_precedence` (`edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs`)
- Related: [multi-certificate-ref.md](multi-certificate-ref.md) (multi-cert *within one* listener — RSA+ECDSA dual-cert selection, intentional, not a collision)
