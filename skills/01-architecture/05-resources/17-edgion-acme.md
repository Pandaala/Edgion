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
4. **Credential targets**: both account and EAB references require an omitted/empty
   group and an omitted/`Secret` kind. The shared ACME target validator is also
   called before handler resolution and background Secret consumption.

### parse

Resolved Secret bytes are **not** written back to the in-memory `EdgionAcme` — `AcmeService` re-fetches via the K8s API during issuance/renewal (`get_or_create_acme_account`). Handler-side resolve is used only for the `ResolvedRefs` condition + `SecretRefManager` registration (so Secret changes trigger requeue).

1. **Clear stale state**: clear the derived attachment plus previous Secret, cross-namespace, and Gateway dependency edges
2. **Resolve `challenge.gatewayRef`**: require a Controller-published Gateway, any required `ReferenceGrant`, and one or more selected `HTTP` listeners; `sectionName` and `port` must both match when both are present
3. **Validate credential targets before resolution**: unsupported account or EAB
   group/kind sets `ResolvedRefs=False/InvalidKind`, skips account lookup and
   Secret dependency registration, and still publishes denial/notification.
   Previous Secret/cross-namespace dependencies were already cleared.
4. **Resolve ACME account Secret** (`spec.privateKeySecretRef`, required):
   - Goes through the credential pipeline (`resource_processor::credential::pipeline::resolve_secret` + `AcmeAccountResolver`)
   - The Secret is allowed to be `Opaque` type, with the required key `accountCredentials` (JSON, i.e., `instant_acme::AccountCredentials`)
   - If the Secret does not yet exist (first-time install / waiting for `AcmeService` write-back), `ResolvedRefs=Unknown/NeedsRegistration` is recorded; once `AcmeService` generates the account it writes the Secret and triggers requeue
5. **Prepare notification**: compare UID/generation, ResolvedRefs status/reason,
   and resolved attachments with the previous processed cache, including denial
   transitions so active work can retire. Reset and recompute the internal
   `notify_after_publish` flag each parse; it is excluded from serde and schemars.

### after_publish

Send the prepared notification only after the new eligibility conditions and attachments
are visible in ServerCache. Never use raw persisted conditions as the deduplication
baseline: a failed generic status CAS must not turn a service status echo into another
notification that resets its retry budget. See the [notification guard](../../04-review/acme/regression-guards/resource-change-notification-gating.md).

### on_delete

Clear SecretRefManager, CrossNsRefManager, and GatewayRouteIndex references.

### update_status

Handler side (`EdgionAcmeHandler::update_status`) **only** manages the Gateway-API-style `Accepted` condition:

- validation_errors empty → `Accepted=True/Accepted`
- validation_errors non-empty → `Accepted=False/Invalid`, with message being the joined errors
- Current code still carries `last_failure_reason` alongside structured status.
  This is pre-release removal debt for old tooling, not a field new consumers
  should read or writers should extend.

**Phase and other ACME lifecycle fields** (`certificate_serial`, `last_renewal_time`,
`secret_name`, etc.) are computed by the background `AcmeService`. The write paths are:

- The handler derives conditions but the generic Merge Patch serializes its whole observed
  status, including nonempty lifecycle/challenge fields. It also updates `last_failure_reason`.
- `update_acme_status` sends lifecycle fields using forced SSA with field manager
  `edgion-acme-controller`. Its freshly built payload has empty conditions and challenges,
  so serde omits those fields.
- `patch_active_challenges` sends an explicit `activeChallenges` array using forced SSA
  with field manager `edgion-acme-challenges`; `[]` explicitly clears the challenges.

The shared `persist_k8s_status` captures the handler input resourceVersion before processing
and includes it as an atomic PATCH precondition. If either service writer succeeds after
that snapshot, the stale handler write fails with 409 instead of rolling back the newer
phase or challenge list. A missing/empty version is rejected locally. One attempt is made;
later source events/requeues can recompute status, but recovery has no time bound.

Skipping an empty list is not a concurrency guarantee: a nonempty observed list is still
serialized. The version guard is also not SSA field ownership or leader fencing. In
particular, `last_failure_reason` remains an overlapping handler/service field; removing
that field or redesigning its ownership is outside this correction. The two specialized
SSA writers remain in place. A blanket conversion of full generic snapshots to forced SSA
would still submit fields belonging to other writers and is not the selected remedy.
See [status snapshot concurrency](../../04-review/status/regression-guards/kubernetes-status-snapshot-concurrency.md).

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
