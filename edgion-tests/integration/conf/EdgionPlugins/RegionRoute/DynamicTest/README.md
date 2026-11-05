# RegionRoute validation over hot reload and management HTTP

`RegionRouteValidation` covers the D0065 integration follow-up using the shared
Controller, Gateway, and test_server. Load `base,HTTPRoute/Basic,EdgionPlugins/base`
before running (the Basic fixtures supply `test-http` and its EndpointSlice):

```bash
target/debug/test_client -g --dynamic-scenario RegionRouteValidation
```

The harness admin tokens must be exported to the test client, including
`EDGION_GATEWAY_ADMIN_TOKEN` for Gateway resource and access-log observations.
The normal `RegionRoute` static suite remains a separate selected verification.

## Fixtures and transitions

`initial/consumers.yaml` defines two consumers (`rrv-small`, modulo 100, and
`rrv-large`, modulo 1000) with required regional setting references, plus `rrv-bad` with an
invalid hash modulo. Both valid consumers refer to `rrv-overlay`. The scenario creates
the setting through generic resource HTTP ingest; its compact, deterministic
phase payloads live in `region_route_validation.rs` alongside the expectations.
Consumers are applied once and their stored specs are checked again at the end.

- The initial table selects `a`; a valid replacement selects `b` for hash 10.
- Before each invalid/absent/inert transition, `b` is observed, so retaining the
  previous good setting cannot pass a Continue assertion.
- Closed-interval overlap, malformed endpoints in an unselected region, self,
  dangling and cyclic failover edges, and an empty table fail standalone validation.
- An upper bound of 999 is valid only for the large consumer; 1000 is valid
  standalone but invalid for both consumers.
- Missing, envelope-disabled, wrong-type and invalid local membership settings
  skip routing under default failOpen. They report unresolved references. Every
  case recovers to `a`. The historical phase name `omitted` now tests a readable
  setting whose local region is absent from its table; omitted required payload
  fields are deserialization errors tested separately.
- The effective HTTP API returns a valid table only for available consumers and
  reports configurationAvailable=false for unavailable consumers, including rrv-bad.
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
Continue must return the shared port-30001 backend response and log the
configuration-unavailable reason without selecting a regional endpoint. Remote targets intentionally retain loopback endpoints: production
address safety rejects the jump (`BackendLocalhostNotAllowed`), and the existing RegionRoute access log proves
the exact selected region and endpoint. This verifies routing intent and preserves
address enforcement; it does not claim successful cross-region network delivery.
No DNS or post-resolution security policy is relaxed.

Cleanup removes only the scenario's `rrv-*` resources and waits for their absence
on both sides, before setup and after success or failure. The invalid consumer sample
is therefore safe even if the static loader preloads the initial directory.

The dedicated failover checks use the setting-scoped preview endpoint followed by
CAS apply with the returned source precondition. Invalid previews leave source
spec/generation unchanged (and resourceVersion unchanged outside Kubernetes).
A successful apply asserts persistence only; the subsequent dynamic phase checks
Controller observation and real Gateway behavior independently. A 409 is a test
failure, never an automatically retried write.
