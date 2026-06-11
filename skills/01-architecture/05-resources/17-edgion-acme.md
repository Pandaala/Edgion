---
name: resource-edgion-acme
description: EdgionAcme resource — ACME automatic certificate issuance, DNS/HTTP-01 challenges, Leader-only execution.
---

# EdgionAcme resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionAcme is Edgion's custom-extension resource that implements automatic certificate issuance via the ACME protocol (Automatic Certificate Management Environment). It supports ACME CAs such as Let's Encrypt, validating domain ownership through HTTP-01 or DNS-01 challenges and automatically issuing and renewing TLS certificates.

## Source code locations

- Controller Handler: `src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_acme.rs`
- ACME Service: `src/core/controller/services/acme/`
- Gateway ConfHandler: `src/core/gateway/services/acme/conf_handler_impl.rs`
- Type definitions: `edgion-resources/src/resources/edgion_acme.rs`

## Controller-side processing

### Spec / Status responsibility split

`EdgionAcmeSpec` carries only **operator input**: `server` / `email` / `privateKeySecretRef` / `domains` / `keyType` / `challenge` / `renewal` / `externalAccountBinding` / `storage` / `autoEdgionTls`. **No controller-managed fields**.

`EdgionAcmeStatus` is the controller's observation channel: in addition to lifecycle fields (`phase`, `certificateSerial`, `certificateNotAfter`, `lastRenewalTime`, `lastFailureReason`, etc.), there is `activeChallenges` — the list of HTTP-01 challenge tokens, written by `AcmeService` via the dedicated fieldManager `edgion-acme-challenges` through the `/status` subresource SSA. The Gateway reads via the `EdgionAcme.active_challenges()` helper (pointing at `status.activeChallenges`) to serve `/.well-known/acme-challenge/<token>`.

`AcmeChallenge` is an internally-tagged enum with `#[serde(tag = "type")]`: `type: http-01` carries `gatewayRef`; `type: dns-01` carries `provider` / `credentialRef` / `propagationTimeout` / `propagationCheckInterval`. The two field groups are mutually exclusive at the type level — there is no illegal "type=http-01 but with dns01 fields" state.

### validate

1. **email**: required, used for ACME account registration
2. **domains**: at least one domain
3. **Wildcard domain check**: a wildcard domain in the form `*.example.com` must use the DNS-01 challenge type
4. **DNS-01 provider check**: only when challenge is DNS-01, check the provider name (cloudflare, alidns supported). HTTP-01 / DNS-01 field completeness is guaranteed at deserialization time by the `AcmeChallenge` tagged enum and does not need a runtime check

### parse

Resolved Secret bytes are **not** written back to the in-memory `EdgionAcme` — `AcmeService` re-fetches via the K8s API during issuance/renewal (`get_or_create_acme_account` / `fetch_dns_credentials`). Handler-side resolve is used only for the `ResolvedRefs` condition + `SecretRefManager` registration (so Secret changes trigger requeue).

1. **Clear stale references**: clear records for this EdgionAcme in SecretRefManager
2. **Resolve ACME account Secret** (`spec.privateKeySecretRef`, required):
   - Goes through the credential pipeline (`resource_processor::credential::pipeline::resolve_secret` + `AcmeAccountResolver`)
   - The Secret is allowed to be `Opaque` type, with the required key `accountCredentials` (JSON, i.e., `instant_acme::AccountCredentials`)
   - If the Secret does not yet exist (first-time install / waiting for `AcmeService` write-back), `ResolvedRefs=False/SecretNotFound` is recorded; once `AcmeService` generates the account it writes the Secret and triggers requeue
3. **Resolve DNS credentials Secret** (DNS-01 mode only):
   - From `dns01.credentialRef`, obtain the Secret reference (`SecretObjectReference` struct, following Gateway API style; this does not go through the unified `SecretMaterialResolver` because the keys vary per provider: cloudflare = `api-token`, alidns = `access-key-id` + `access-key-secret`)
   - Register the reference in SecretRefManager; validation failure / DecodeError, etc. map to the `ResolvedRefs` condition
4. **Notify ACME Service**: call `notify_resource_changed()` to notify the background ACME Service of a resource change, triggering the certificate issuance/renewal flow

### on_delete

Clear SecretRefManager references.

### update_status

Handler side (`EdgionAcmeHandler::update_status`) **only** manages the Gateway-API-style `Accepted` condition:

- validation_errors empty → `Accepted=True/Accepted`
- validation_errors non-empty → `Accepted=False/Invalid`, with message being the joined errors
- The `last_failure_reason` text field is also kept for legacy tools that still read it

**phase and other ACME lifecycle fields** (`certificate_serial`, `last_renewal_time`, `secret_name`, etc.) are not written by the handler; they are updated independently by the background `AcmeService` via Server-Side Apply (field manager `edgion-acme-controller`). The three writers' field ownership does not overlap:
- Handler writes `status.conditions` (Merge Patch)
- AcmeService writes `status.phase` and related fields (SSA, fieldManager `edgion-acme-controller`; in the payload, `conditions` is skipped due to `skip_serializing_if = "Vec::is_empty"` and will not clear what handler wrote; `activeChallenges` is similarly skipped due to `Vec::is_empty` and is not touched by this SSA)
- AcmeService writes `status.activeChallenges` (SSA, fieldManager `edgion-acme-challenges`, an independent path; only has values during HTTP-01 in flight)

There is a theoretical brief rollback window where SSA + handler Merge Patch coexist — "while the processor handles an event, the watch has not yet reflected AcmeService's latest phase". The same applies to `activeChallenges`: the handler's Merge Patch payload serializes the in-memory `EdgionAcmeStatus` whole and writes it to `/status`, including `activeChallenges` (when non-empty) deserialized from the watch event. If the handler is processing an RV=N event while AcmeService writes a new round of challenges at RV=N+1, the handler's Merge Patch will roll `activeChallenges` back to the RV=N version. In practice:

- AcmeService pushes once at HTTP-01 phase 2 and clears once at phase 6, with no intermediate writes in between, so a normal issuance does not trigger the race;
- The only thing that triggers it is the rare combination of "operator changes spec while a flow is in progress";
- Self-heal path: on the next watch event, the handler re-reads the latest `activeChallenges` and writes it back; the AcmeService retry mechanism is the safety net.

**Root-cause fix** (TODO, framework-level): the handler's status persistence should be changed to SSA + an explicit fieldManager (e.g., `edgion-acme-handler`), with K8s field ownership rejecting cross-manager overrides. This requires changing the generic `persist_k8s_status` path and is out of scope here.

## ACME Service

ACME certificate issuance is handled by an independent background Service:

- **Leader-only execution**: certificate issuance runs only on the Leader node, avoiding duplicate issuance from multiple nodes
- **HTTP-01 challenge**: requires the HTTP port to be reachable; the ACME CA verifies the domain via an HTTP request
- **DNS-01 challenge**: creates a TXT record via DNS provider APIs to verify the domain (Cloudflare, Alibaba Cloud DNS supported)
- **Certificate storage**: on successful issuance, creates or updates a Kubernetes Secret containing the certificate and private key
- **Automatic renewal**: monitors certificate validity and re-issues automatically before expiry

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionAcme → Secret | Secret | `spec.privateKeySecretRef` + `AcmeAccountResolver` | ACME account private key (`accountCredentials`), written by `AcmeService` on first registration; protected by `Zeroizing<String>` |
| EdgionAcme → Secret | Secret | `spec.challenge.dns01.credentialRef` | DNS provider API credentials (provider-specific keys) |
| EdgionAcme → Secret | Secret | create/update (SSA) | Issued certificate (`tls.crt` / `tls.key` / `ca.crt`) is written into `storage.secretName` |
| EdgionAcme ← SecretRefManager | Secret | cascading requeue | Reprocess on changes to the account / DNS credentials Secret |
| EdgionAcme → Gateway/EdgionTls | indirect | via the certificate Secret | The issued certificate Secret is referenced by Gateway/EdgionTls |
