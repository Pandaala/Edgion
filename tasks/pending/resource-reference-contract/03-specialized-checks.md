# Reference design: two-round focused checks

## Scope and authority

The user requested two further checks in each of three directions: reference
identity versus ReferenceGrant, implemented reference coverage, and Gateway API
conformance impact. These checks supplement the three preceding general Astra
reviews; none is implementation approval or fresh runtime/conformance evidence.
Only task documents are changed. The source task remains open.

Two read-only Astra reviewers exchange directions in round 2. Reviewer A checks
Grant consistency and conformance in round 1, then coverage in round 2. Reviewer
B checks coverage in round 1, then Grant consistency and conformance in round 2.
Each reads original repository rules, relevant knowledge and current code. Root
verifies findings and serially amends the specification and field manifest.

## Round results

| Direction | Round 1 | Round 2 on amended documents |
| --- | --- | --- |
| Grant consistency | Reviewer A: Issues Found, G1-01 through G1-03; corrected | Reviewer B: final LGTM on the amended documents |
| Reference coverage | Reviewer B: Issues Found, C1-1 through C1-4; corrected | Reviewer A: C2-01/C2-02 wording/policy precision findings corrected; final re-read LGTM |
| Conformance impact | Reviewer A: Issues Found, K1-01 and K1-02; corrected | Reviewer B: final LGTM on the amended documents |

All three directions completed both rounds. Final reviewers found no remaining
design/coverage blocker within the audited scope. Round 2 corrected two additional
precision defects; those corrections were re-read before the final verdicts.
Existing product decisions and implementation verification remain separate gates.

## Decisive findings and resolution

| ID | Evidence and gap | Document correction |
| --- | --- | --- |
| G1-01 | Controller config/mod.rs owns the default-on enable_reference_grant_validation switch; route/Gateway/mirror checks consult it, credential pipelines and CredentialInjector source checks do not | Spec 4.1/7 and manifest preserve per-consumer policy, mandatory identity/dependency checks, no new bypass, and no conformance claim while disabled |
| G1-02 | Upstream ReferenceGrant requires revocation; Gateway serving certificates rebuild from current outcomes and tls_pingora rejects incomplete material without SecretStore fallback | Spec 5/7 separates standard revocation after denied-generation publication from named custom retention contracts and existing in-flight behavior |
| G1-03 | GrantStore.authorization_revision, LinkSys bootstrap inputs, CredentialInjector bindings and credential-manager async rechecks reconstruct authorization identities | Spec 3.3/4.4/5/7 requires the same canonical query and exact matcher for permission and revision capture/rechecks, preserving matching-Grant UID/resourceVersion publication fences |
| C1-1 | CredentialInjector parse/readonly and credential_manager/secret_io consume generated Secret owner/source/consumer bindings beyond authored input refs | Manifest and spec inventory consumer-local generated outputs, source-UID memory identity, exact managed metadata, CAS cleanup/publication, expiry and leadership |
| C1-2 | EndpointSlice Service label association and DNS output naming/ownerReferences are consumed; malformed label/namespace fallbacks differ between planes | Manifest and spec inventory association/output categories, preserve old/new pool rebuilds and existing fallback differences, gate any harmonization |
| C1-3 | Generic string/annotation rows omit exact Redis/Webhook and Controller-process DNS fields and conceal Gateway/route grammar differences | Manifest replaces catch-alls with exact owning fields and annotation keys; separates explicit object/string refs, aliases and Redis counter namespaces |
| C1-4 | allowedRoutes Namespace selectors read NamespaceStore labels; service-owned PKI/Lease and synthetic etcd bootstrap fields are outside the advertised resource-reference migration | Manifest classifies selector policy and evidence-backed exclusions; spec bounds its no-unchecked-lookup requirement to frozen reference consumers |
| K1-01 | Standard/Experimental parentRefs CEL differs; current SchemaValidator evaluates OpenAPI only and its GVK registry selects a static schema profile | Spec 4.6/7 freezes channel-specific scoped list/union/CEL constraints without claiming null-view admission parity or introducing general discovery/admission |
| K1-02 | Harness permits CRD mismatch; saved v1.6.2 Standard and Experimental reports have different results, and older reports lack bundle metadata | Spec 7 requires actual installed artifacts/channel and runtime revision evidence, separates targeted/full-profile runs and retains known baseline failures |
| C2-01 | Gateway plugin-list parser splits exactly comma/space/tab/LF, then trims tokens; the prior ASCII-whitespace wording also implied embedded CR/form-feed/vertical-tab separators | Manifest specifies the exact separators and spec 7 adds distinguishing grammar fixtures; no parser behavior change |
| C2-02 | Credential readers share managed-binding/expiry checks but do not perform manager leadership, exact-owner or mutation CAS checks | Manifest and spec distinguish read/readonly parity from operation-specific manager fences; identity sharing does not harmonize acceptance or add handler I/O |

Grant matching also explicitly requires exact namespaces, omitted to.name as the
only name wildcard, from/to conjunction within one Grant, and OR across whole
Grants. Terms from different Grants cannot be composed to authorize a reference.
Custom core shorthand is converted at its finite declaring-field adapter before
this exact matcher; standard Grant terms never receive alias conversion.

## Coverage evidence and limits

Round 1 searched all five production source trees for reference, identity,
binding, target, selector and namespace/name definitions and concrete consumers;
compared resource schemas, annotations and direct Kubernetes API operations;
traced material, ConfigData, conditions, plugin stages, outputs and scoped
instances. It found no AST completeness proof. Generic JSON/template embeddings
and each stage-specific variant still require the frozen implementation audit.
An unlisted field never inherits convenience from a shared wire type.

Exclusions are explicit in the field manifest: unsupported features, unused
EndpointSlice endpoint targetRef, generic metadata, service-owned PKI/Lease,
synthetic non-resource placeholders and network/file/request identifiers. Those
exclusions grant no new lookup, authorization or write feature.

## Verification boundary

These are code/schema/primary-source design checks. No production changes,
Cargo tests, integration runs, installed-CRD admission, live conformance run,
commit or push occurs in this phase. Future implementation acceptance must cover
the scoped cases in spec 7. The unrelated existing agent-document check failure
remains recorded in [05-issues.md](05-issues.md).

## Supporting evidence

- [ReferenceGrant v1.6.2 semantics and revocation](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/referencegrant_types.go)
- [Channel-specific parent list constraints](https://github.com/kubernetes-sigs/gateway-api/blob/v1.6.2/apis/v1/shared_types.go)
- [Controller-owned Grant policy](../../../skills/04-review/resources/intentional-design/reference-grant-validation-controller-scoped.md)
- [Credential generated output and mutation fences](../../../edgion-controller/src/services/credential_manager/secret_io.rs)
- [EndpointSlice pool ownership transitions](../../../skills/04-review/backends/regression-guards/endpoint-slice-ownership-transitions.md)
- [Current source-schema validation limits](../../../skills/08-kubernetes/01-gateway-api/07-version-support/03-validation-current-state.md)
- [Saved conformance report boundaries](../../../edgion-tests/conformance/reports/README.md)
