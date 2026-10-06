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

One Controller-local `services::acme::validate_secret_references` checks both
fields. The handler invokes it during validation and before credential resolution;
unsupported targets skip account reads and Secret dependency registration while
still publishing status. Old Secret and cross-namespace edges are cleared before
the check. Notification preparation still runs so a published denial can retire
active work. Invalid targets take diagnostic precedence over a Gateway error.
Existing account-data errors retain their previous Gateway-error precedence.

The scheduler checks raw-source metadata independently of apparently eligible
processed conditions. `get_or_create_acme_account` and `fetch_eab_hmac` validate
before constructing/using the Secret API, so restoration, registration, and
credential persistence cannot bypass the target check.

## Alternatives and limits

Do not enforce this only in the CRD: FileSystem, etcd, and typed Kubernetes inputs
converge on the same handler. The CRD descriptions document the supported targets
without adding enum restrictions that would bypass lenient status publication.
Do not add a generic pipeline outcome variant or alter unrelated credential
consumers solely to repair ACME.

Valid account references retain their existing ReferenceGrant check and
`Unknown/NeedsRegistration` bootstrap outcome for absent Secrets/keys. This fix
does not establish EAB authorization parity, validate Secret names/data syntax,
or prove a credential disclosure or cross-namespace bypass. It also does not
change Gateway token serving: active challenge status remains governed by the
existing listener-proof and expiry rules, while the Controller denies new work.
The shared core-group alias contract remains a separate
[investigation](../../../../tasks/todo/core-api-group-alias-contract.md); its
current `core` alias is not evidence of the Gateway API Secret reference default.

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

Verification of the final code:

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
