# gw-infra-06: Gateway materializes Etcd LinkSys connections that no data-plane feature consumes

- **Category**: over-design
- **Severity**: P3 — downgraded from P2 by independent verification
- **Scope**: local
- **Location**: `edgion-gateway/src/link_sys/providers/etcd/{mod.rs,apply.rs}`, `link_sys/runtime/store.rs:223,469-485,633-655,777-...` (`ProviderKind::Etcd`, `apply_entry` Etcd arm, `get_etcd_client`, `etcd_runner_exists`, `list_etcd_clients`, `health_check_all_etcd`), `link_sys/runtime/health_probe.rs`, `edgion-gateway/src/api/mod.rs:784-821,1189-...` (only consumers: `/api/v1/testing/link-sys/etcd/*`)

## Summary
Every Gateway replica turns each `LinkSys` of type Etcd into a persistent runner. The runner resolves the
basic-auth Secret and outbound TLS, dials the etcd cluster with keepalive, runs health probes with backoff,
and publishes health transitions. No Gateway plugin, route, or runtime path reads an etcd client. The only
callers of `get_etcd_client` in `edgion-gateway` are the integration-testing admin endpoints. The Etcd
provider exists in the Gateway only so that testing endpoints can exercise a client that the Controller
already uses (`conf_center/etcd` reuses `EtcdLinkClient` from `edgion-common`).

## Current behavior (evidence)
- `grep get_etcd_client|EtcdLinkClient` outside `link_sys/` and `api/` in `edgion-gateway/src` returns nothing.
  Redis (rate limiting), Webhook (key/forward-auth), HTTPDNS (`backends/mod.rs:394`) and ES/Kafka/OTLP
  (access-log senders) all have data-plane consumers. Etcd does not.
- `store.rs:469-485`: once the credentials are ready, `PersistentRunner::<EtcdApply>::spawn` creates the
  runner. `apply.rs` fingerprints auth material and TLS, and builds the client through the common provider.
  `health_probe.rs` keeps probing it.
- `api/mod.rs:784-821` exposes `/api/v1/testing/link-sys/etcd/{health,ping,get,put,...}` only when
  `is_integration_testing_mode()`.
- Any Etcd LinkSys the operator creates for the control plane (Controller conf center or leader election) is
  therefore also dialed from *every* Gateway pod, with its credentials resolved on the data plane. N
  Gateway replicas create N long-lived etcd connections and N probe loops. That is load on the
  control-plane backing store and a least-privilege violation: data-plane compromise yields etcd
  credentials and live sessions.

## Industry comparison
Envoy, Kong and APISIX data planes do not hold the control plane's backing-store client. APISIX's data plane
reads etcd only because etcd *is* its config transport. Kong hybrid-mode data planes never receive DB
credentials. Edgion's architecture already places etcd behind the Controller (gRPC conf_sync), so the
Gateway-side Etcd provider has no product role.

## Impact
Maintenance cost: provider apply, fingerprinting, credential readiness, health, admin surface and tests.
Avoidable connections to etcd and secret exposure on every Gateway replica.

## Suggested direction
1. Remove `ProviderKind::Etcd` / `EtcdApply` from the Gateway LinkSys runtime, and remove the `testing/link-sys/etcd`
   admin endpoints. Move Etcd client integration tests to `edgion-common` (the client's owner) or the
   Controller.
2. Have the Controller stop shipping Etcd-type LinkSys (and its referenced Secrets) to Gateways at all. This
   overlaps with `arch-01` (conf-sync broadcasts all Secrets). A per-kind "data-plane relevant" flag on LinkSys
   variants gives a natural filter.
3. If a future Gateway plugin really needs etcd, reintroduce the provider together with that consumer.

## Notes
Checked `skills/04-review/link-sys/` (SKILL, `intentional-design/provider-owned-runtime-and-simple-removal.md`,
`regression-guards/etcd-configuration-and-message-boundaries.md`, `link-sys-hot-reload-reuse-and-retirement.md`).
They define *how* the Etcd provider behaves (SDK-owned dialing, health probes, retirement), but none states a
Gateway data-plane consumer or a reason to run it in the Gateway. Related to
`tasks/improve-issues/arch-01-conf-sync-broadcasts-all-secrets-to-every-gateway.md`. This finding adds the
live-connection and backing-store load aspect, which is specific to Etcd. Pre-release: no compatibility
shim needed.

## Verification (independent)

- **Verdict**: DOWNGRADED — P2 → P3.
- **Correction**: the claim that "any Etcd LinkSys the operator creates for the control plane" is also dialed from every Gateway is unfounded. The Controller conf center builds its etcd client from bootstrap config (`edgion-controller/src/conf_mgr/conf_center/etcd/center.rs:69`, `EtcdLinkClient::from_bootstrap`), not from a LinkSys CR. An Etcd LinkSys therefore has no production consumer anywhere. The cost is mainly dead surface, plus credentials and connections only when someone creates such a CR.
- `get_etcd_client` / `list_etcd_clients` / `health_check_all_etcd` / `etcd_runner_exists` are referenced only in `link_sys/` and `api/mod.rs`. The `api/mod.rs:784-821` routes are `/api/v1/testing/link-sys/etcd/*`.
- `link_sys/runtime/store.rs:469-485` still spawns a `PersistentRunner<EtcdApply>` per Etcd LinkSys, so the removal recommendation stands as a cleanup.

## Additional evidence (wave 3)

The cost per Etcd LinkSys on every Gateway replica is larger than one long-lived connection. The client
built by the Gateway runner is tagged with `with_gateway_apply_meta` (`edgion-gateway/src/link_sys/providers/etcd/apply.rs:77`),
which sets `sample_endpoint_coverage = true` (`edgion-common/src/link_sys_core/providers/etcd/client.rs:182-191`).
With that flag, `init` (`client.rs:241`) and every 10-second health tick (`client.rs:269,311`) call
`sample_endpoint_live` (`client.rs:476`). That function opens a **new** `etcd_client::Client` to each configured
endpoint separately, then issues a `status()` RPC, and drops the client. With `auth` configured, each of those
connects also performs an etcd `Authenticate` RPC, which costs a server-side bcrypt check and creates a token.
This is per replica × per endpoint × every 10 s, in addition to the monitor's own `status()` probe on the
shared client. The Controller conf-center client (`from_bootstrap`) does not set the flag, so this sampling
exists only on the Gateway side, where nothing consumes the client. This strengthens recommendation 1
(remove the Gateway Etcd provider). If the provider is kept, the coverage sample should reuse the shared
client's member list (`member_list` / per-endpoint `status` over the existing channel) instead of
re-dialing and re-authenticating.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID — Severity P3 unchanged.
- The `link_sys/` diff is concentrated in the Redis provider rewrite; the Etcd provider is untouched. `link_sys/runtime/store.rs:469-482` still spawns `PersistentRunner::<EtcdApply>` per Etcd LinkSys.
- Outside `link_sys/`, `get_etcd_client` / `list_etcd_clients` / `health_check_all_etcd` / `etcd_runner_exists` are still referenced only by the testing routes in `api/mod.rs:784-821` and helpers at `:1189-1250`.
- `edgion-common/src/link_sys_core/providers/etcd/client.rs:182-191,240,309,476` still enable per-endpoint re-dial sampling for the gateway client. The only etcd review-page change is the SDK bump to 0.21.1 and token refresh; it names no Gateway consumer.
