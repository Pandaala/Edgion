# res3-01: EdgionAcme writes its certificate Secret into any namespace and overwrites any existing Secret there

- **Category**: design-error
- **Severity**: P1 (security)
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_acme.rs:128-139` (`AcmeStorageConfig.secret_name` / `secret_namespace`)
  - `edgion-resources/src/resources/refs/acme_fields.rs:188-203` (`AcmeOutputRefs::certificate` only checks name syntax)
  - `edgion-resources/src/resources/refs/acme_fields.rs:137-154,217-219` (`validate_api_metadata` / `validate_certificate_metadata`)
  - `edgion-controller/src/services/acme/service.rs:1813-1886` (`create_or_update_cert_secret`, forced SSA)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_acme.rs:77-121` (handler `validate`)
  - `cicd/deploy/kubernetes/controller/rbac.yaml:31-37` (ClusterRole: create/update/patch/delete on all Secrets)

## Summary
An `EdgionAcme` in namespace A chooses `storage.secretNamespace` and `storage.secretName` freely.
The leader Controller then force-applies a `kubernetes.io/tls` Secret with that name in that namespace.
It uses its cluster-wide Secret write permission. No ReferenceGrant is required. The code does not
check that an existing Secret was created by this ACME resource. A user who may only create
`EdgionAcme` in their own namespace can therefore create or overwrite TLS Secrets in any namespace,
including other tenants' namespaces and the Edgion system namespace.

## Current behavior (evidence)
- `AcmeOutputRefs::certificate()` (`acme_fields.rs:188-203`) resolves the destination as
  `spec.storage.secret_namespace.or(metadata.namespace)`. It only runs `ObjectIdentity::checked`,
  which validates DNS-label/subdomain syntax.
- `EdgionAcmeHandler::validate` (`handlers/edgion_acme.rs:77-121`) checks the account Secret
  references, email, domains, wildcard rejection and durations. It does not check the output
  destination. The account private-key Secret is protected by ReferenceGrant (`:415-436`), but the
  *write* destination is not.
- `create_or_update_cert_secret` (`service.rs:1813-1886`) reads the current Secret. It then calls
  `outputs.validate_certificate_metadata(&current.metadata)`. That function (`acme_fields.rs:137-154`)
  only compares the returned object's name/namespace with the expected target, so it always passes.
  The Controller then runs
  `api.patch(name, &PatchParams::apply("edgion-acme-controller").force(), &Patch::Apply(secret))`.
  This takes ownership of `data.tls.crt`, `data.tls.key`, `type` and the ACME labels. The existing
  `uid`/`resourceVersion` are copied in, so the write is a deliberate in-place overwrite.
- The comment at `service.rs:1872-1876` describes adopting an existing Secret as intended. That is the
  same-namespace `kubectl create secret tls` → ACME migration case. Nothing limits this adoption to the
  owner's namespace or to Secrets already marked `edgion.io/managed-by=acme`.
- The Controller's ClusterRole grants `create/update/patch/delete` on all Secrets (`rbac.yaml:31-37`).
- The credential manager handles the same kind of output with an explicit check.
  `secret_io::classify_ownership` (`services/credential_manager/secret_io.rs:222-242`) returns
  `Conflict` unless the Secret carries the managed-by label, consumer markers and an exact owner
  reference. ACME has no such guard.

Attack sketch: a tenant controls `evil.example` and may create `EdgionAcme` in `team-a`. They set
`domains: [evil.example]`, `storage.secretNamespace: team-b` and `storage.secretName: prod-tls`.
Once HTTP-01 succeeds, `team-b/prod-tls` holds a certificate for `evil.example`. Every listener and
`EdgionTls` that serves `team-b/prod-tls` now presents the wrong certificate, which is an outage. The
same works against any `kubernetes.io/tls` Secret in the system namespace that the Controller or
Gateway consumes. If the target type differs, Kubernetes refuses the type change, but the attacker
can still create new Secrets anywhere.

## Industry comparison
- cert-manager: `Certificate.spec.secretName` is always written in the Certificate's own namespace.
  There is no cross-namespace output field. cert-manager also refuses to take over a Secret whose
  `cert-manager.io/certificate-name` annotation names another Certificate.
- Gateway API: cross-namespace *reads* need a ReferenceGrant in the target namespace. No standard
  resource lets a namespaced object make a controller *write* into another namespace.

## Impact
Any principal with `create` on `EdgionAcme` (normally an app team) can break other tenants' TLS or
plant Secrets in other namespaces. The Controller acts as a confused deputy with its cluster-wide
Secret permission. Because the Controller renews periodically, it re-applies the overwrite on every
renewal.

## Suggested direction
1. Remove `storage.secretNamespace` (pre-release, no compatibility burden) and always write to the
   EdgionAcme namespace. This matches cert-manager. Consumers in other namespaces already reach the
   Secret through ReferenceGrant-checked reads.
2. If cross-namespace output must stay, require a ReferenceGrant in the destination namespace
   (`from: edgion.io/EdgionAcme in A`, `to: core/Secret [name]`). Enforce it in the handler (status
   `ResolvedRefs=False`) and again at write time in `create_or_update_cert_secret`.
3. In both cases, add the credential manager's ownership rule. Overwrite an existing Secret only if it
   has no data or already carries `edgion.io/managed-by=acme` and
   `edgion.io/acme-resource=<this namespace/name>` (or an owner reference to this EdgionAcme). Any
   other Secret should fail with a `Conflict` status instead of being force-applied. The documented
   migration from a hand-made TLS Secret can be an explicit opt-in annotation on that Secret.

## Notes
- Checked `skills/04-review/acme/*`. `secret-reference-target-identity.md` covers the *account* Secret
  and states that it "does not ... prove a credential disclosure or cross-namespace bypass". No page
  covers the output destination. `acme-service-shape.md` is about service structure.
- `tasks/pending/resource-reference-contract/02-field-manifest.md:285` lists
  `storage.secretName/secretNamespace` as a "Core Secret write destination" whose fencing must be
  preserved. It does not discuss authorization of the destination, so this is new analysis.
- The `CLAUDE.md` SSA-force rule (every `Patch::Apply` must be forced) is why this cannot be fixed by
  dropping `force`. The ownership check has to happen before the apply.

## Verification (independent)

Verdict: CONFIRMED. Severity P1 kept.
- `AcmeOutputRefs::certificate()` (`edgion-resources/src/resources/refs/acme_fields.rs:186-200`) takes `spec.storage.secret_namespace` verbatim; the only field use in the whole tree is that line (grep), so no handler or ReferenceGrant check guards the write destination.
- `create_or_update_cert_secret` (`edgion-controller/src/services/acme/service.rs:1813-1886`) calls `validate_certificate_metadata`, which only re-compares name/namespace with the same target (`acme_fields.rs:137-154,217-219`), then force-applies over the existing Secret, copying its `uid`/`resourceVersion`. No `edgion.io/managed-by` / `acme-resource` ownership check precedes the overwrite.
- Controller ClusterRole grants create/update/patch/delete on all Secrets (`cicd/deploy/kubernetes/controller/rbac.yaml:31-37`), so the confused-deputy write is authorized cluster-wide; `kubernetes.io/tls` Secrets in the system namespace (e.g. conf_sync leaf certs) are reachable targets.
- No `skills/04-review/acme/*` page covers the output destination; `secret-reference-target-identity.md` is about the account Secret only.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1
- `acme_fields.rs` changed only `kube::api::ObjectMeta`→`kube::core::ObjectMeta` and reformatting; `AcmeOutputRefs::certificate` (`:188-203`) still takes `secret_namespace` verbatim and `validate_certificate_metadata` (`:217-219`) still only re-compares identity. Line refs updated.
- `services/acme/`, `handlers/edgion_acme.rs`, `edgion_acme.rs` and `rbac.yaml` are unchanged; no new `skills/04-review/acme` page.
