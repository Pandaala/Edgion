# ctrl-svc-07: Controller-side OAuth2 token acquisition makes data-plane requests depend on control-plane uptime

- **Category**: design-error (with over-design)
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/services/credential_manager/service.rs:285-510` (leader-only 1 s wake loop), `:608-810` (`acquire_and_publish`)
  - `edgion-controller/src/services/credential_manager/scheduler.rs:7-23` (`EXPIRY_MARGIN`, refresh at min(rotation, lifetime/2))
  - `edgion-controller/src/services/credential_manager/memory.rs` (persist-off store lives only in the leader process)
  - `edgion-controller/src/services/credential_manager/secret_io.rs` (640 lines: per-consumer generated Secrets, 12 annotations, owner refs, CAS, adoption, sweep)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/credential_injector.rs:256-277` (`memory_material`: missing/expired → `ResolvedRefs` error)
  - `edgion-gateway/src/plugins/http/credential_injector/plugin.rs:37-49`, `:93-96` (expired `validUntil` → constructor returns `None` / request rejected 500)
  - `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:2659`, `:2483-2495` (`Ok(None)` → plugin set sealed fail-closed)

## Summary
For `LinkSys type: credentialSource`, the leader Controller performs the OAuth2 client-credentials
exchange, then ships the bearer token to gateways embedded in each consumer `EdgionPlugins`
(`resolvedCredential` with `validUntil`). Gateways never refresh tokens themselves. The gateway
injector rejects requests once `validUntil` passes, and a gateway that restarts or relists with an
expired token builds no injector and seals the plugin set fail-closed. Therefore any Controller
outage (crash loop, leader election stuck, conf_sync broken, apiserver outage blocking the Lease
fence) longer than roughly half a token lifetime turns into a **data-plane outage** for every route
using a managed credential. This breaks the usual gateway property that the data plane keeps serving
on last-known-good config when the control plane is down. On top of this coupling sits a large
publication subsystem (~2.9k lines incl. two publication modes, generated-Secret ownership/adoption,
CAS annotations, leader-term fencing, startup sweeps) to move a short-lived token from one process to
another.

## Current behavior (evidence)
- Acquisition happens only when `backend.is_leader()` and the Lease fence holds
  (`service.rs:300-302`, `:625-639`); transient failures leave the source without a new token.
- Refresh is scheduled at `min(rotation.interval, lifetime/2)` with a 30 s expiry margin
  (`scheduler.rs:11-23`); a provider token of 1 h therefore gives roughly 30 minutes of control-plane
  outage tolerance, a 5-minute token about 2 minutes.
- Persist-off tokens live only in `ControllerRuntime.credential_memory` of the current leader
  (`memory.rs`); `memory_material` returns `MissingKeys` when the entry is absent or
  `valid_until_mono` has passed (`credential_injector.rs:261-268`). A newly elected leader starts
  with an empty store and must acquire before consumers resolve again.
- Gateway: `CredentialInjector::create` returns `None` when `resolvedCredential` is absent or its
  `validUntil` is already past (`plugin.rs:38-47`); the runtime treats `Ok(None)` as fail-closed
  (`plugin_runtime.rs:2483-2495`). At request time an expired deadline returns 500
  (`plugin.rs:94-96`).

## Industry comparison
- **Envoy** `envoy.http.injected_credentials.oauth2` (credential injector, client-credentials
  flow): each Envoy fetches and refreshes its own token from the token endpoint; the control plane
  only delivers the client id/secret via SDS. Control-plane loss does not stop token refresh.
- **Kong** (`openid-connect` client-credentials / `upstream-oauth`), **APISIX** (`openid-connect`),
  **Apigee/Tyk** all acquire upstream tokens in the data plane, with a local cache per node.
- Istio's rule of thumb: anything with a lifetime shorter than an acceptable control-plane outage
  must be refreshable by the proxy itself (that is why workload certs are fetched by the agent).

## Impact
- Availability: a control-plane incident of tens of minutes causes upstream auth failures for all
  managed-credential consumers; gateway restarts during the incident fail immediately.
- Leader failover adds a window in which persist-off consumers cannot resolve.
- Complexity: two publication modes with different HA support matrices (`persist: false` rejected
  in `all-serve`, `persist: true` rejected outside Kubernetes), generated Secrets in tenant
  namespaces (one reason the controller needs cluster-wide Secret write RBAC, see ctrl-svc-01).

## Suggested direction
Options:
1. (Recommended) Move token acquisition to the gateway: the controller ships the
   `credentialSource` config plus the resolved bootstrap client secret (it already ships resolved
   Secrets via conf_sync with redaction adapters); each gateway runs a small refresher (shared per
   LinkSys key, single-flight, refresh at half-life with jitter, bounded response read, egress
   policy via `build_outbound_client`). Token issuance then survives control-plane outages; the
   controller keeps validation/status only. The generated-Secret publication path and
   `credential_memory` can be deleted (pre-release, no compatibility burden).
2. If central acquisition is a hard requirement (e.g. issuer rate limits per client), keep it but
   (a) let gateways fall back to a local refresh when `validUntil` is near and no newer token has
   arrived, and (b) persist tokens only via one mechanism (drop one of the two publication modes).
3. At minimum, document the outage-tolerance bound (≈ lifetime/2) and alert on
   `credential source token age > lifetime/2`.

## Notes
- Checked `skills/04-review/security/intentional-design/credential-copy-retention.md` (copy
  ownership, not availability), `link-sys/regression-guards/credential-source-single-active-memory.md`
  (memory-store shape), `skills/01-architecture/05-resources/18-link-sys.md`. None evaluates the
  control-plane/data-plane availability coupling. Related: `tasks/pending/resource-reference-contract/`
  mentions CredentialSource references only.
- The leader-failover window claim follows from the per-process memory store; its exact length
  depends on initial-processing order and should be measured before choosing option 2.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-gateway/src/plugins/http/credential_injector/plugin.rs:37-48`: `create` returns `None` when `resolved_credential` is absent or `valid_until` already passed.
- `plugin.rs:93-96`: request-time expiry → `ErrResponse { status: 500 }`.
- `edgion-controller/src/services/credential_manager/scheduler.rs:7-22`: refresh at `min(rotation, lifetime/2)` with 30 s margin, leader-only.
- No review page evaluates the control-plane/data-plane availability coupling (`credential-copy-retention.md`, `credential-source-single-active-memory.md` cover other aspects).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `services/credential_manager/`, controller `resolve_credential/credential_injector.rs` and `edgion-gateway/src/plugins/http/credential_injector/` unchanged since `feature-08-30`.
- `plugin_runtime.rs` changed by one line (not in the cited `Ok(None)` fail-closed path); acquisition is still leader-only with refresh at `min(rotation, lifetime/2)`.
