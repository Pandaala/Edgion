---
id: ARCH-125
status: candidate
kind: behavior-defect
priority: P2
partition: PART-409
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---


# Correlate each BackendTLSPolicy CA reference with its resolved material identity

## Current ownership and concrete witness

BackendTlsPolicyHandler::parse_policy resolves each declared caCertificateRef from the policy's local namespace. Secret references use ordinary_secret_material; ConfigMap references use configmap_ca_material, which preserves fetched identity and stamps the canonical edgion.io/controller-ca-source: ConfigMap marker. The ordinary Secret sanitizer removes that reserved marker. These existing producers distinguish the two supported resource kinds inside the current Vec<Secret> transport without another schema or cache.

However, all_runtime_refs_resolved at handlers/backend_tls_policy.rs:236–247 and update_policy_status at511–529 both call is_ca_resolved(spec, &ca_ref.name). The helper at787–800 matches only metadata.name and a nonempty ca.crt payload. It never correlates the reference's kind or local namespace with that material. The duplicate check at169–179 distinguishes (group, kind, name), so a Secret and ConfigMap with the same name are accepted as distinct references.

Consider an existing Service target and a valid local Secret ns/ca containing a usable CA bundle, alongside an absent ConfigMap ns/ca. A policy declares both group-empty references, kinds Secret and ConfigMap, name ca, with no System CA selection. This is a supported source shape: the current v1.6.2 CRD permits up to eight atomic local references; the handler explicitly supports both kinds. Parse registers both separate recovery edges but obtains only the Secret material. The name-only check returns true for both slots. The handler therefore publishes resolved_target_class and reports ResolvedRefs=True for a current owned Gateway ancestor. An existing ConfigMap lacking ca.crt produces the same alias. The inverse witness also holds: ConfigMap material can incorrectly satisfy a missing Secret reference.

The downstream Gateway does not independently reconstruct these source objects. BackendTLSPolicyStore::is_valid_candidate checks the Controller target marker and any nonempty resolved CA payload at222–278. ConfHandler publishes the received resource, and the supported BoringSSL peer builder consumes the supplied bundle. With a valid surviving bundle, these gates do not repair the falsely complete source classification. This is an exact static source witness, not an executed deployment incident, unauthorized reference or system-trust fallback claim.

## Required contract and bounded improvement

The committed standard v1.6.2 CRD at229–306 and905–982 requires an absent or misconfigured CA reference to produce ResolvedRefs=False with the affected reference and cause; connections using an invalid CA reference must fail. The current Controller also deliberately requires every declared runtime CA reference to resolve before publishing its target-class marker. A material from another kind cannot establish that requirement merely because the names coincide.

Replace the existing name-only predicate with one bounded, reference-correlated classification used by both status and runtime eligibility. Correlate the supported canonical group/kind, effective policy-local namespace and name with the material identity and existing exact source marker; retain the payload-presence check. Reuse the canonical marker constants and actual fetched-object wrapper/sanitizer contract. Preserve the field's current omitted-namespace semantics rather than adding a raw metadata spelling restriction. A private cold helper in this handler is sufficient; no new reference taxonomy, Common proof framework, runtime registry, public constructor or serialized field is required.

Counting material entries alone is insufficient: it does not establish which reference each entry satisfies, and must not replace identity or existing duplicate-reference validation. Re-reading all stores during update_status would create another acquisition boundary and could classify a different generation. Correlating the prepared material keeps status and eligibility aligned with this reconciliation's actual outputs.

Preserve strict standard local-group rules, supported kinds, current per-reference dependencies before lookup, clear-before-resolve, CA/client-axis separation, System selection, existing error precedence and NoValidCACertificate aggregation. Keep current typed target arbitration, section semantics and invalid-candidate occupancy; this correction must not delete an invalid policy to expose plaintext/system-CA fallback. Leave native PEM preparation, full bundle parsing, profile identity, immutable publication and captured request lifetime unchanged. Add no metrics, traffic logs or handler IO.

## Verification and existing-task relationship

The full current BackendTLSPolicy handler and actual 678-line handler test body were read. Tests cover target typing, conflict timestamps, dropped/current EBTP requeues and foreign ancestors, but not this same-name missing-kind CA witness. Actual CA helper tests verify exact ConfigMap identity/marker production, whitespace-empty rejection and removal of forged markers from ordinary Secrets. The canonical Common outbound TLS material-pair validator also enforces kind provenance and namespace/name identity for its distinct owning fields; the BTP path does not call that loan-specific validator.

Future tests should drive actual parse and update_status with same-name Secret/ConfigMap references: both present, each missing independently, each missing/empty ca.crt independently, and both ref orders. Assert the incomplete reference is reported, no falsely complete target marker is published, and actual Gateway preparation retains the invalid-policy refusal outcome when no other valid candidate exists. Use actual Secret/ConfigMap event recovery edges to show repair restores eligibility and deletion withdraws it. Retain ordinary one-reference, System, strict group/kind, duplicate-validation and no-op/rotation controls. No tests, builds, applications or network operations were executed.

This is a concrete refinement of the existing pending resource-reference-contract R8 BTP material-binding boundary: tasks/pending/resource-reference-contract/07-progress.md:2513–2524 explicitly leaves separate BTP/EBTP material bindings pending, and 02-field-manifest.md:88 classifies this field as LocalCa. That broad pending migration is not evidence that the current alias is correct or already fixed. Record this precise witness and bounded acceptance there when implementing; do not treat this review as permission to run the whole migration. ARCH017 concerns cold native TLS preparation, ARCH106 final ancestor capacity and ARCH055 timestamp transport precision; their causes and fixes are distinct. The historical EBTP requeue page's legacy Service-key translation prose is stale against the current typed BTP base key, and does not justify rediscovering that obsolete gap.
