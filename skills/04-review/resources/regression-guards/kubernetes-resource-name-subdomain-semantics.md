---
name: kubernetes-resource-name-subdomain-semantics
description: Review Kubernetes object-name validation, long resource owners, and native reference grammar boundaries.
knowledge_type: regression-guards
---

# Kubernetes resource names use subdomain total-length semantics

## Conclusion and scope

`resources/common/kubernetes_name.rs::is_dns_subdomain` implements the Kubernetes
resource-name predicate: lowercase ASCII alphanumeric components with internal
hyphens, separated by dots, under the caller's total-length bound. Kubernetes's
253-byte subdomain rule has no separate 63-byte component limit. Empty components,
uppercase, leading/trailing hyphens, slashes and overlong total names remain invalid.

The shared helper serves actual resource owners, checked object references and API
groups. It does not validate actual DNS hostnames. Namespace uses the separate
63-byte label validator. Native Gateway/BTP material ObjectName references retain
their separate nonempty/253-character grammar, including non-DNS spellings. Exact
group/kind selection, required fields, namespace policy and Grant matching do not
change when the name predicate accepts a previously rejected valid identity.

Service references and the DNS annotation's Service owner currently use the generic
checked identity. Those callers already allowed dotted names up to 253 total bytes;
correcting the shared helper removes their component cap. This does not establish
Kubernetes Service metadata admission for such names or remove a dedicated
Service-63 validator. Actual Service metadata, DNS hostname/SNI/certificate names,
section labels and other explicit label validators have separate contracts.

Health-check TLS validation retains its existing local composition: 253 total
bytes and at most 63 bytes per component, with the shared `is_dns_label` predicate
also validating explicit namespaces. ARCH-059 removes only the duplicate lexical
implementation; substituting `is_dns_subdomain` would broaden health acceptance.
The component cap on health Secret/ConfigMap references is observed behavior
preserved by this refactor, not a newly approved product requirement. Whether
those references should accept broader Kubernetes object names remains a separate
semantic decision. Health hostname identity, numeric-host rejection and TLS
error precedence remain independently enforced by `tls_validation_error`.

## Rationale and evidence

Kubernetes v0.36.1 `apiextensions-apiserver/pkg/registry/customresource/validator.go`
selects `NameIsDNSSubdomain` for custom-resource metadata. That delegates to
`apimachinery/pkg/util/validation/validation.go::IsDNS1123Subdomain`, which applies
only the total-length bound and a regex without component-length quantifiers.
Its parallel content-validator tests explicitly accept one 253-byte component.
Official [Kubernetes core validation](https://github.com/kubernetes/kubernetes/blob/v1.36.1/pkg/apis/core/validation/validation.go)
also assigns Secret and ConfigMap names this predicate; Namespace and Service use
distinct label rules.

S-R1-01 exposed the false negative after actual-owner reference validation reached
HTTP/gRPC cold compilation and native TLS material acquisition. A valid 64-byte
HTTPRoute name failed `RouteOwner::identity`, and HTTP compilation returned no
units. Gateway/BTP `OwnerSnapshot::source` rejected the same valid owner before
material acquisition, even with a short valid Secret target. The correction reuses
the canonical byte predicate and bounds components by the already checked total,
without allocation, regex compilation or a source-only alternative validator.

The BTP `edgion.io/client-certificate-ref` option now delegates to that helper
instead of its private duplicate. Its original feature/options contract specifies
a same-namespace Secret name, not an independent 63-byte component restriction.
Trimming, fixed core/Secret identity, local scope, no slash, option absence and the
existing error text remain. This bounded correction authorizes no cleanup of other
private validators or broadening of unrelated string-reference grammars.

## Verification and limits

Regression tests cover 63/64/253/254 boundaries, malformed syntax, namespace limits,
native ObjectName separation, actual plugin targets, Service reference behavior,
Gateway/BTP owner material pairing, and trimmed BTP client Secret names. Controller
tests acquire through actual owner tokens, distinguish readonly from dependency
registration, and observe zero reads/edges for invalid owners. HTTP tests compile
backend-bearing routes and retain the long owner's attribution.

Test source alone is not an execution result. Fresh commands, preservation evidence,
independent reviews and acceptance belong to the
[resource-reference task progress](../../../../tasks/pending/resource-reference-contract/07-progress.md).
Unit/material pairing and cold route compilation do not establish live admission,
conf_sync, traffic, certificate usability, TLS handshakes or conformance. Those
remain separate F2 acceptance obligations. Revisit this rule if Kubernetes changes
name semantics, a new caller validates real DNS names, or a field gains an explicit
independent product grammar.

## Resolution references

- S-R1-01 / R12a: canonical resource subdomain validation and bounded BTP parser reuse.
- ARCH-059: health-specific composition reuses the label predicate; focused
  `resources::health_check` tests cover reference/hostname bounds and error order.
