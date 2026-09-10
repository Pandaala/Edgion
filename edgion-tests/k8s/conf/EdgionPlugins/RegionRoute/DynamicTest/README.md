# RegionRoute validation over hot reload and management HTTP

`RegionRouteValidation` covers the D0065 integration follow-up using the shared
Controller, Gateway, and test_server. The Kubernetes fixture routes use the
deployment-owned `edgion-test-server` Service, so selective dynamic runs do not
depend on the static `HTTPRoute/Basic` fixture:

```bash
target/debug/test_client -g --dynamic-scenario RegionRouteValidation
```

The harness admin tokens must be exported to the test client, including
`EDGION_GATEWAY_ADMIN_TOKEN` for Gateway resource and access-log observations.
The normal `RegionRoute` static suite remains a separate selected verification.

## Fixtures and transitions

`initial/consumers.yaml` defines two consumers (`rrv-small`, modulo 100, and
`rrv-large`, modulo 1000) with a safe local `base` table, plus `rrv-bad` with an
invalid base. Both valid consumers refer to `rrv-overlay`. The scenario creates
the overlay through generic resource HTTP ingest; its compact, deterministic
phase payloads live in `region_route_validation.rs` alongside the expectations.
Consumers are applied once and their stored specs are checked again at the end.

- The initial table selects `a`; a valid replacement selects `b` for hash 10.
- Before each invalid/absent/inert transition, `b` is observed, so retaining the
  previous good overlay cannot pass a base-fallback assertion.
- Closed-interval overlap, malformed endpoints in an unselected region, self,
  dangling and cyclic failover edges, and an empty table fail standalone validation.
- An upper bound of 999 is valid only for the large consumer; 1000 is valid
  standalone but invalid for both consumers.
- Missing, envelope-disabled, wrong-type and regions-omitted overlays use the
  base. Omitted regions inherit successfully; missing/disabled/wrong-type and
  invalid consumer tables report unresolved references. Every case recovers to `a`.
- The effective HTTP API must return exactly one correct table per valid
  consumer and exclude `rrv-bad`.
- Dedicated failover HTTP writes reject self/dangling/cyclic candidates and
  malformed existing tables with 422. Clearing an already absent field still
  validates the full table. The stored spec and generation must remain unchanged;
  the FS run also checks resourceVersion (Kubernetes status writes can change it).
- Legal writes form `a -> b -> c`. Requests selecting `a` must stop at `b`.

## Synchronization and observation limits

All transitions and cleanup use `ScenarioCtx::wait_phase`, with no fixed sleep
or additional topology. Each phase independently checks Controller status and
its observedGeneration, the Gateway's published raw resource, the effective API,
and fresh traffic. Controller status alone is not a Gateway acknowledgement.
Polling establishes eventual agreement after each serial mutation; it does not
claim a transaction across the separate management reads.

Traffic uses hash 10 (or direct region `a` for failover) and a fresh trace ID.
Base fallback must return the shared port-30001 backend response and log local
selection. Remote targets intentionally retain loopback endpoints: production
address safety rejects the jump (`BackendLocalhostNotAllowed`), and the existing RegionRoute access log proves
the exact selected region and endpoint. This verifies routing intent and preserves
address enforcement; it does not claim successful cross-region network delivery.
No DNS or post-resolution security policy is relaxed.

Cleanup removes only the scenario's `rrv-*` resources and waits for their absence
on both sides, before setup and after success or failure. The invalid base sample
is therefore safe even if the static loader preloads the initial directory.
