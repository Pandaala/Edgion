# Design and verification issues

## Astra reviewer capacity

Initial creation attempts from the primary agent and an inventory agent returned
agent thread limit reached. Existing inventory agents performed explicitly named
Sol cross-reviews instead. After the user requested continuation, an independent
gpt-6-astra reviewer was successfully created. The initial capacity blocker is
resolved. Astra round 1 found design issues; round 2 reviewed corrections and gave
LGTM for the design. Round 3 found per-prefix TLS restrictions, omitted fixed/log/
output identities, and insufficient LinkSys lifecycle boundaries. The latest
spec and field manifest correct these. Final round-3 re-review returned LGTM
with no remaining design blockers. Three actual Astra rounds are complete;
this is design-quality review, not implementation/product approval or runtime
verification.

## Additional focused review

The user requested two rounds in each of three directions after the general
reviews. Two Astra reviewers exchanged directions for independent second-round
checks. Grant consistency, reference coverage and conformance impact all reached
final LGTM after nine first-round amendments and two second-round precision
corrections. See [03-specialized-checks.md](03-specialized-checks.md).
No remaining focused design blocker was found; this does not close the product
decision gates or establish implementation/admission/conformance verification.

## EndpointSlice fallback policy

Status: explicit implementation-impact gate if harmonization is proposed.
Current Controller and Gateway absent Service-label/namespace fallbacks differ.
The manifest inventories the actual behavior and preserves both policies during
identity extraction. An intentional behavior correction requires separate
impact review before implementation; no implicit change is approved here.

## Standard backend kinds versus connection modes

Status: explicit implementation decision gate, revised after Astra round 1.
Current ServiceClusterIp/ServiceExternalName/
ServiceEndpointSlice/EndpointSlice routing spellings are not registered resource
kinds. Mapping their actual target to Service fixes lookup authorization but
does not make their use in standard backendRef.kind conformant. The strict
shared module must not silently legitimize that behavior. The specification must
gate the separate Edgion connection-mode surface or strict rejection before a
routing migration is implemented; spec section 4.5 now does so. Ordinary
omitted/Service references stay supported. No mode carrier is approved here.

## Cluster-scoped material authorization

Status: explicit implementation decision gate, revised after Astra round 1.
EdgionGatewayConfig has no source namespace, while
standard ReferenceGrant.from.namespace requires a namespace. Current Secret
fetch invents default and ConfigMap fetch invents empty. An actual Cluster
identity must not inherit either authorization identity. Spec section 4.7
proposes administrative authority for exactly the EGC outbound CA/client fields.
Approval must explicitly include the material-selection authority of EGC writers;
current runtime behavior is unchanged until that decision and implementation.

## Existing repository agent-document check failure

The spec-only run of python3 cicd/checks/validate_agent_docs.py failed with:

~~~text
stale src/ ref in skills/04-review/ai-routing/regression-guards/ai-cost-weighted-request-output-bound-parsing.md:16
-> edgion-gateway/src/ai/protocols/openai/request.rs
~~~

This is an unchanged knowledge page outside the reference task. The failure
prevents a clean repository-wide document-gate claim; it is not evidence of a
runtime reference defect. Targeted new-document link/structure/language checks
are separate verification. Repair the owning page and rerun the check before
claiming the repository gate passed; no finding is closed in this design phase.

## Current F1 implementation audit (2026-10-10)

At committed HEAD 3cd8d681c75a4bdbb67d68b37eeb461bbf2c266b, fresh Astra
rounds found two remaining source/API acceptance blockers. Earlier sections
record design-phase history and are not a current runtime acceptance verdict.

- **S-R1-01, P2:** actual Route/Gateway/BackendTLSPolicy owner validation adds a
  63-byte per-component limit absent from Kubernetes DNS1123 object-name
  admission. Valid 64..253-byte CR names can fail before routing or TLS
  acquisition. Independently confirmed in the Standard round-1/round-2 reports
  under tmp/resource-reference-contract/f1-final-standard-round[12]-3cd8d681-20261010.md.
  R12a now passes two independent final reviews, 2321 distinct focused tests,
  and fourteen warning-free workspace gates. Its canonical predicate correction
  preserves namespace, actual DNS-host, native ObjectName and field-specific
  contracts; the BTP local Secret option reuses that predicate. Committed as
  f6b71a5217ce868e198b315ced6b8cff4c6f1b45 after both scoped commit reviews;
  all thirteen blobs match the reviewed manifest. Fresh long-name API admission
  and traffic remain F2.
- **F1-COV-R1-01:** ManagedOutboundClient::from_parts and Common
  build_rustls_client_config retain obsolete detached transport entry points
  after their product callers migrated. Their exclusive raw branches and test
  callers require an independently reviewed removal/migration batch. This is
  an API-completion gap, not a demonstrated ordinary-ingress Grant bypass.
  Round-1 coverage and independent round-2 source inspection confirm the residual;
  preserve live canonical HTTP/TLS conversion and process/bootstrap boundaries.
  R12b now removes the four obsolete entries and migrates all 34 test calls.
  Two fresh independent Astra final reviews pass the frozen twenty-path scope;
  803 distinct focused tests and fourteen warning-free root gates pass. Live
  canonical cores and original test assertions remain. The exact local commit
  is the remaining batch gate; fresh F2 acceptance is separate.

Grant semantic inspection has found no additional confirmed authorization bug;
source review does not prove live revocation or conformance. Documentation also
needs current/historical reconciliation and precise ConfigData Cluster wording:
a matching Grant is unnecessary, while some readers still consult its store.
Fresh admission, dynamic traffic and full Standard conformance remain open.
