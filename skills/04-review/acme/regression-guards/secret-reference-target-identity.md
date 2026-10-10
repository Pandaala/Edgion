---
name: acme-secret-reference-target-identity
description: Review ACME account and EAB reference target validation before Secret consumption.
knowledge_type: regression-guards
---

# ACME credential references preserve target identity

## Conclusion and scope

EdgionAcme supports only core Kubernetes Secrets for `privateKeySecretRef` and
`externalAccountBinding.keySecretRef`. An omitted or empty group means the core
API group; an omitted kind means `Secret`; an omitted namespace means the owner
namespace. Explicit non-empty groups (including `core`), empty kinds, and kinds
other than the case-sensitive `Secret` are unsupported. Unsupported targets
produce `Accepted=False` and `ResolvedRefs=False/InvalidKind` and cannot be
interpreted as a same-name Secret, even when that Secret exists.

This is an ACME-specific correction, not a policy change for every consumer of
the shared SecretObjectReference or the generic credential pipeline. It adds no
non-Secret source support and does not broaden the HTTP-01-only product scope.

## Rationale and evidence

The [Gateway API SecretObjectReference contract](https://gateway-api.sigs.k8s.io/reference/api-spec/1.5/spec/#secretobjectreference)
defines group/kind as target identity and requires invalid identity to be
reported through conditions. The vendored Gateway API standard CRD carries the
same contract. The user selected those semantics for EdgionAcme when resolving
`acme-secret-reference-target-validation`; Gateway API does not itself specify
EdgionAcme's custom status reason. `InvalidKind` reuses Edgion's existing reason.

Previously, the account handler's generic pipeline and the service's account/EAB
Secret APIs ignored group/kind. ACME feature documentation called these reserved
metadata, but that did not establish an accepted choice to silently consume a
different resource type. The field-coverage correction alone had not resolved
the independent runtime contract.

The runtime-independent `AcmeRefs` facade borrows the actual resource and supplies
separate account/EAB loans with the canonical strict Secret contract. The
Controller-local `validate_secret_references` is only its diagnostic adapter.
Source and target name/namespace must also pass canonical strict identity checks;
explicit empty namespaces are invalid. These custom-resource name checks are not
the native Gateway API ObjectName grammar. Omitted owner namespace means default.

The handler validates both fields, then resolves only the account through the
bound pipeline. Invalid credential fields have no credential read/dependency
side effects; the existing independent HTTP-01 Gateway dependency/Grant path
still runs. Type-target errors retain their diagnostic priority; account material
errors retain the existing Gateway-error precedence. Pending account references
retain ordinary Secret and cross-namespace recovery edges. EAB is not a new
handler acquisition/dependency or a new ReferenceGrant policy.

Account local-cache acquisition alone supports the finite default bare/qualified
selector. Two candidates are ambiguous; unique candidates must match their actual
raw metadata. Semantic Secret notifications include EdgionAcme so missing, deleted,
ambiguous and repaired account sources reprocess it. Other consumers do not gain
this projection. Account Grant enforcement does not inherit native Gateway or
EdgionTls's disabled-validation exceptions.

The background service derives API reads from actual fields. Kubernetes API
responses require explicit matching namespace/name before payload consumption;
raw-cache omitted-namespace defaults do not apply to returned API metadata.
Account persistence takes only the distinct account loan, never an EAB loan,
and derives its API path, payload identity and source labels/owner reference.
Initial and refreshed snapshots must match that field before UID/RV CAS or
concurrent-credential adoption. The original UID, deletion check, conflict retry,
valid-account adoption, missing-key bootstrap and corruption rejection remain.
The attempt's existing leader/source/serving fences still establish operation
authority; field identity by itself does not.

## Alternatives and limits

Do not enforce this only in the CRD: FileSystem, etcd, and typed Kubernetes inputs
converge on the same handler. The CRD descriptions document the supported targets
without adding enum restrictions that would bypass lenient status publication.
Do not add a generic pipeline outcome variant or alter unrelated credential
consumers solely to repair ACME.

Valid account references retain their existing ReferenceGrant check and
`Unknown/NeedsRegistration` bootstrap outcome for absent Secrets/keys. This fix
does not establish EAB authorization parity, change account data syntax,
or prove a credential disclosure or cross-namespace bypass. It also does not
change Gateway token serving: active challenge status remains governed by the
existing listener-proof and expiry rules, while the Controller denies new work.
The shared custom core alias does not override these strict ACME fields. Generated
certificate Secret and auto-EdgionTls output identity remains separately tracked;
account read/persistence closure does not establish generated-output write authority.

## Verification and re-evaluation

Focused handler tests cover omitted/explicit core targets, unsupported groups,
empty/wrong-case/other kinds, both reference fields, and present/absent account
Secrets after YAML/JSON decoding. A real processor transition covers old-edge
cleanup, publication/notification, and correction recovery. Service tests cover
eligible defaults and unsupported raw targets, and use an in-process Kubernetes
client service to assert zero API calls on unsupported account/EAB targets.

The existing `AcmeGatewayRefOwnership` dynamic scenario includes invalid-account,
invalid-EAB, and recovery phases. It polls Controller-computed current conditions
transported to the Gateway, while preserving the existing serving assertions.

Historical verification of the original target-type correction (not fresh verification
of the actual-field migration):

- `cargo test -p edgion-controller --lib acme`: 50 passed, including the handler,
  service, account Secret IO, and ACME client tests. The earlier separately filtered
  service and handler runs passed 31 and 14 tests respectively.
- `cargo check --workspace --all-targets` and
  `cargo clippy --workspace --all-targets` passed. The new nonminimal-boolean lint
  was corrected and both commands rerun; the final Clippy run emitted no warnings.
- Formatting, agent-doc consistency, SSA force, metrics inventory, Pingora source
  policy, Gateway API transform contract, unit-test layout, and English-only
  guards passed.
- Canonical `prepare.sh` rebuilt the binaries before the existing shared-topology
  startup script loaded `Gateway/AcmeGatewayRefOwnership/DynamicTest/initial`.
  `test_client -g --dynamic-scenario AcmeGatewayRefOwnership` passed; report:
  `tmp/integration_testing/testing_20261006_233214/acme-secret-target.log`.
  Both invalid-target/recovery sequences and the prior listener/revocation/expiry
  phases completed. Owned runtime processes were cleaned up after the run.
- Independent read-only code and knowledge review returned LGTM, including an
  incremental review after the lint correction.

The dynamic run uses the FileSystem Controller mode and verifies condition
transport and recovery, not live Kubernetes admission or CA registration. No
full-workspace test run or live-CA verification is claimed.

Re-evaluate if ACME gains another credential source, changes reference defaults,
moves credential consumption outside the current service, or alters lenient
admission and scheduler eligibility contracts.

## Resolution references

- `acme-secret-reference-target-validation` — silent target-type substitution
  corrected under the user-selected Gateway API reference semantics.
