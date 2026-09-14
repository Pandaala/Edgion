---
name: resource-edgion-acme
description: EdgionAcme resource — ACME automatic certificate issuance, HTTP-01 challenges, Leader-only execution.
---

# EdgionAcme resource

> **Common flow**: see [00-resource-flow.md](00-resource-flow.md)

EdgionAcme is Edgion's custom-extension resource that implements automatic certificate issuance via the ACME protocol (Automatic Certificate Management Environment). It supports ACME CAs such as Let's Encrypt, validating domain ownership through HTTP-01 challenges and automatically issuing and renewing TLS certificates.

## Source code locations

- Controller Handler: `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_acme.rs`
- ACME Service: `edgion-controller/src/services/acme/`
- Gateway ConfHandler: `edgion-gateway/src/services/acme/conf_handler_impl.rs`
- Type definitions: `edgion-resources/src/resources/edgion_acme.rs`

## Controller-side processing

### Spec / Status responsibility split

`EdgionAcmeSpec` carries operator input plus one internal transport field,
`resolvedListenerAttachments`. The latter is Controller-owned,
`#[schemars(skip)]`, absent from the CRD, cleared before every reconcile, and
serialized only so the Gateway can enforce serving ownership.

`EdgionAcmeStatus` is the controller's observation channel: in addition to lifecycle fields (`phase`, `certificateSerial`, `certificateNotAfter`, `lastRenewalTime`, `lastFailureReason`, etc.), there is `activeChallenges` — the list of HTTP-01 challenge tokens, written by `AcmeService` via the dedicated fieldManager `edgion-acme-challenges` through the `/status` subresource SSA. The Gateway reads via the `EdgionAcme.active_challenges()` helper (pointing at `status.activeChallenges`) to serve `/.well-known/acme-challenge/<token>`.

`AcmeChallenge` accepts only `type: http-01` with `gatewayRef`. Other challenge types and unknown challenge fields are rejected. For DNS-01/wildcard issuance, use an external issuer such as cert-manager and reference its TLS Secret.

### validate

1. **email**: required, used for ACME account registration
2. **domains**: at least one domain
3. **Wildcard domain check**: wildcard domains are rejected because built-in ACME supports HTTP-01 only.

### parse

Resolved Secret bytes are **not** written back to the in-memory `EdgionAcme` — `AcmeService` re-fetches via the K8s API during issuance/renewal (`get_or_create_acme_account`). Handler-side resolve is used only for the `ResolvedRefs` condition + `SecretRefManager` registration (so Secret changes trigger requeue).

1. **Clear stale state**: clear the derived attachment plus previous Secret, cross-namespace, and Gateway dependency edges
2. **Resolve `challenge.gatewayRef`**: require a Controller-published Gateway, any required `ReferenceGrant`, and one or more selected `HTTP` listeners; `sectionName` and `port` must both match when both are present
3. **Resolve ACME account Secret** (`spec.privateKeySecretRef`, required):
   - Goes through the credential pipeline (`resource_processor::credential::pipeline::resolve_secret` + `AcmeAccountResolver`)
   - The Secret is allowed to be `Opaque` type, with the required key `accountCredentials` (JSON, i.e., `instant_acme::AccountCredentials`)
   - If the Secret does not yet exist (first-time install / waiting for `AcmeService` write-back), `ResolvedRefs=Unknown/NeedsRegistration` is recorded; once `AcmeService` generates the account it writes the Secret and triggers requeue
4. **Notify ACME Service**: notify only after the Gateway attachment is resolved and the account result is not a hard error

### on_delete

Clear SecretRefManager, CrossNsRefManager, and GatewayRouteIndex references.

### update_status

Handler side (`EdgionAcmeHandler::update_status`) **only** manages the Gateway-API-style `Accepted` condition:

- validation_errors empty → `Accepted=True/Accepted`
- validation_errors non-empty → `Accepted=False/Invalid`, with message being the joined errors
- Current code still carries `last_failure_reason` alongside structured status.
  This is pre-release removal debt for old tooling, not a field new consumers
  should read or writers should extend.

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
- **Certificate storage**: on successful issuance, creates or updates a Kubernetes Secret containing the certificate and private key
- **Automatic renewal**: monitors certificate validity and re-issues automatically before expiry

## Cross-resource relationships

| Direction | Target resource | Mechanism | Notes |
|---------|---------|---------|------|
| EdgionAcme → Secret | Secret | `spec.privateKeySecretRef` + `AcmeAccountResolver` | ACME account private key (`accountCredentials`), written by `AcmeService` on first registration; protected by `Zeroizing<String>` |
| EdgionAcme → Secret | Secret | create/update (SSA) | Issued certificate (`tls.crt` / `tls.key` / `ca.crt`) is written into `storage.secretName` |
| EdgionAcme ← SecretRefManager | Secret | cascading requeue | Reprocess on changes to the account Secret |
| EdgionAcme → Gateway | Gateway | `GatewayRouteIndex` + resolved attachment proof | Reprocess on listener, ownership, publication-proof, or deletion changes |
| EdgionAcme → ReferenceGrant | ReferenceGrant | `CrossNsRefManager` | Required for a cross-namespace `gatewayRef`; changes re-resolve the attachment |
| EdgionAcme → Gateway/EdgionTls | indirect | via the certificate Secret | The issued certificate Secret is referenced by Gateway/EdgionTls |
