---
name: mirror-uses-separate-http-client-bypasses-lb-tls-policy
description: Use when reviewing findings that RequestMirrorPlugin sends shadow traffic via a standalone reqwest::Client instead of the cluster machinery — bypasses the active LB policy (always round-robin, ignores EWMA/LeastConn/ConsistentHash), bypasses BackendTLSPolicy (port==443 heuristic, no mTLS client cert), unlike Envoy request_mirror_policy where the mirror is a first-class cluster reference; covers why routing the mirror through EdgionBackend/get_peer is a wide-impact feature, not a narrow review fix.
status: accepted-tradeoff
finding_id: route-04
closed: 2026-06-14
---

# Request Mirroring Uses a Separate reqwest Client (Bypasses LB Policy + BackendTLSPolicy) — DESIGN-TRADEOFF (accepted)

## Conclusion

`RequestMirrorPlugin` (`edgion-gateway/src/plugins/http/request_mirror/plugin.rs`) sends the
shadow copy of a request via a standalone `reqwest::Client` rather than through the
`EdgionBackend` / `get_peer` upstream machinery used by the primary request. As a result the
mirror path diverges from the primary path in three ways:

1. **LB policy bypass** — `resolve_target` (`plugin.rs:135`) calls `select_roundrobin_backend_key`,
   which is plain round-robin; the service's configured EWMA / LeastConn / ConsistentHash policy
   does not apply to mirror traffic.
2. **BackendTLSPolicy bypass** — the scheme is a port heuristic `port == 443 → https`
   (`plugin.rs:128`, already marked `// TODO(Phase 2)`); a mirror target requiring mTLS never
   receives a client certificate, so the backend silently rejects the mirror connection.
3. **Per-plugin connection pool** — the `reqwest::Client` is a per-instance field built in `new()`
   (`plugin.rs:26,49-54`); each `EdgionPlugins` reload rebuilds it. (This pool/client-sharing
   sub-aspect is the separate sibling finding F-017; see Reference Cases.)

Health-check and circuit-breaker ejection ARE honored for the mirror path
(`select_roundrobin_backend_key` → `is_backend_healthy`, `backends/selector.rs`), correcting the
original finding's claim that they are ignored.

Fix suggestions of the form "route the mirror through `EdgionBackend` / `get_peer`", "make the
mirror a first-class Pingora upstream request like Envoy's `request_mirror_policy`", "query
`BackendTLSPolicy` and apply the mTLS client cert in the mirror path", or "thread the active LB
policy into `resolve_target`" are **not accepted** as review-issue fixes — that is a large
cross-layer feature (backends + BackendTLSPolicy lookup + LB-policy selection + client-cert
assembly), which belongs in `tasks/todo/` via brainstorming, not a narrow low-risk patch.

## Core Rationale

**1. The proper fix is a wide-impact cross-layer feature, not a narrow bug**

Envoy's baseline (the finding's reference) makes the mirror a *cluster reference*, so shadow
traffic transparently inherits LB, health, outlier detection, and cluster TLS. Replicating that in
Edgion means routing the mirror through the same `select_http_backend` / `get_peer` path the
primary request uses: BackendTLSPolicy-aware service lookup, LB-policy-aware endpoint selection,
and client-certificate assembly for mTLS targets. That spans `backends`, the TLS-policy layer, and
the LB selector — exactly the wide-impact profile the review-issue scope gate pushes into
`tasks/todo/`, with real design surface (how the mirror borrows the primary's resolved peer, how
mTLS client certs are sourced for a fire-and-forget shadow request) that needs a design pass.

**2. The blast radius is shadow traffic only**

The mirror is fire-and-forget: `run_request` spawns the send task and immediately returns
`GoodNext` (`plugin.rs:235-316`); failures surface only in `mirror_log` + a metric and never affect
the primary response. So the BackendTLSPolicy bypass degrades the *mirror*, not production traffic —
it is a silent shadow-path failure, not a request-correctness or security hole on the live path.
LB-policy divergence likewise only changes how shadow load is distributed; round-robin for shadow
traffic is an acceptable simplification. This is consistent with a medium-severity gap, not a
critical one.

**3. The BackendTLSPolicy gap is an acknowledged Phase-2 item, not an oversight**

The port-heuristic scheme already carries an explicit `// TODO(Phase 2): Use BackendTLSPolicy to
determine TLS instead of port heuristic.` comment at `plugin.rs:128`. The limitation is recorded at
the call site; it is a deferred feature, not an undiagnosed defect, and re-filing it as a bug does
not change the disposition.

## Fix Suggestions Not Accepted

- "Route the mirror through `EdgionBackend` / `get_peer` (issue's Suggested direction)" — large
  cross-layer feature; promote to `tasks/todo/` and design via brainstorming.
- "Query `BackendTLSPolicy` in `resolve_target` and build a client-cert-bearing client" — needs the
  per-service client-cert/CA/SNI assembly (the "pool of clients keyed on the differentiator" shape
  noted in the F-017 entry); cross-layer, not a narrow fix.
- "Thread the active LB policy (EWMA/LeastConn/ConsistentHash) into the mirror selector" —
  meaningless in isolation from the above; shadow-traffic round-robin is an accepted simplification.

## Re-evaluation Triggers

Re-open this decision only if:

- The "mirror as first-class cluster reference" feature is prioritized — then it is promoted to
  `tasks/todo/` and designed via brainstorming (BackendTLSPolicy lookup + LB-policy selection +
  mTLS client cert), not closed here.
- A concrete operator scenario shows mirror targets routinely require mTLS in production and the
  silent rejection is causing real operational pain (would raise the BackendTLSPolicy sub-gap above
  shadow-only severity).
- Gateway API standardizes mirror-target TLS/LB semantics that Edgion must conform to.

## Reference Cases

- Finding `route-04-mirror-uses-separate-http-client` (`tasks/review-envoy/`), 2026-06-14 —
  ⚠️ accepted tradeoff; closed without code change.
- Sibling pool/client-sharing sub-aspect: F-017,
  [../cpu-memory/cases/request-mirror-reqwest-client-per-plugin-instance.md](../cpu-memory/cases/request-mirror-reqwest-client-per-plugin-instance.md).
  NOTE: that entry describes a `get_mirror_http_client()` singleton fix; as of 2026-06-14 the
  current source still carries the per-plugin `client` field (`plugin.rs:26,49-54`) and no
  `get_mirror_http_client` exists in `common/http_client.rs`, so the pool-fragmentation sub-aspect
  is live in code — that F-017 record is stale relative to the source.
- Same review-set family of accepted architecture tradeoffs:
  [no-per-route-global-plugin-override-or-exclude.md](no-per-route-global-plugin-override-or-exclude.md),
  [filter-iteration-control-sync-phase-tradeoff.md](filter-iteration-control-sync-phase-tradeoff.md),
  [body-filter-read-only-by-design.md](body-filter-read-only-by-design.md).
- Source: `edgion-gateway/src/plugins/http/request_mirror/plugin.rs:115-146` (`resolve_target`,
  round-robin + port-heuristic scheme), `:235-316` (fire-and-forget spawn);
  `edgion-gateway/src/backends/selector.rs` (`select_roundrobin_backend_key` → `is_backend_healthy`).
</content>
</invoke>
