# Design and verification issues

## Current disposition (2026-10-11)

- S-R1-01 is source-closed by R12a (`f6b71a52`); F1-COV-R1-01 is
  source/API-closed by R12b (`6b5adb0a`). R13 documentation reconciliation
  committed as `0fdd67f4`, with both affected final cross-checks passed.
- The mixed route/policy generation correction committed as `6f566a8ae` after
  dual static/execution reviews, 635 distinct executed passes, fourteen root
  checks and unchanged Redis/retained-listener live scenarios. Existing manager
  reachability and eventual convergence across independent resources remain.
- F2-A conformance passed with fresh images from `6f566a8ae` on Standard v1.6.2:
  HTTP Core 37/Extended 30, GRPC Core 15, TLS Core 20, all with zero reported
  failures/skips. Independent evidence reviews verified schemas, images,
  full declared features and the shared fresh runner. Historical network and
  Standard failures remain evidence for their earlier revisions, not blockers
  for this completed phase. Exact scope is in the dated reports and progress.
- The task-specific F2 admission, compatibility, dynamic reference/Grant traffic,
  long-name TLS and source-mode parity matrices remain open. Conformance does
  not replace them. Amend the pending CredentialInjectorDynamic test plan with
  its generation-sensitive status and actual runtime phase marker before coding.
- EndpointSlice Controller/Gateway fallbacks remain deliberately distinct.
  Keep the original alias issue until full acceptance and final whole-task review.

See [07-progress.md](07-progress.md) for current evidence and remaining work.
Historical records below retain their original revision scope.

## Historical design and verification record

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
No remaining focused design blocker was found. Those reviews did not themselves
close implementation or live acceptance; the later accepted decisions and scoped
implementation results are summarized in the current disposition above.

## EndpointSlice fallback policy

Retained policy: separate fallback behavior is preserved. A future harmonization
would require an explicit implementation-impact review.
Current Controller and Gateway absent Service-label/namespace fallbacks differ.
The manifest inventories the actual behavior and preserves both policies during
identity extraction. An intentional behavior correction requires separate
impact review before implementation; no implicit change is approved here.

## Standard backend kinds versus connection modes

Historical decision gate, revised after Astra round 1 and subsequently resolved
by the approved recommendations in 00-context.md.
Current ServiceClusterIp/ServiceExternalName/
ServiceEndpointSlice/EndpointSlice routing spellings are not registered resource
kinds. Mapping their actual target to Service fixes lookup authorization but
does not make their use in standard backendRef.kind conformant. The strict
shared module must not silently legitimize that behavior. The design required
a decision between a separate connection-mode surface and strict rejection. The
accepted implementation chose strict rejection; no new mode carrier was added. Ordinary
omitted/Service references stay supported. No mode carrier is approved here.

## Cluster-scoped material authorization

Historical decision gate, revised after Astra round 1 and subsequently resolved
by the approved recommendations in 00-context.md.
EdgionGatewayConfig has no source namespace, while
standard ReferenceGrant.from.namespace requires a namespace. At the design
baseline, Secret fetch invented default and ConfigMap fetch invented empty.
An actual Cluster identity must not inherit either authorization identity. Spec
section 4.7 proposed administrative authority for exactly the EGC outbound CA/client fields.
The later approved implementation uses that exact administrative selection with
explicit target namespaces. The invented default/empty source identities described
here are historical defects, not the current EGC authorization contract.

## Existing repository agent-document check failure

The spec-only run of python3 cicd/checks/validate_agent_docs.py failed with:

~~~text
stale src/ ref in skills/04-review/ai-routing/regression-guards/ai-cost-weighted-request-output-bound-parsing.md:16
-> edgion-gateway/src/ai/protocols/openai/request.rs
~~~

This historical failure prevented a clean document-gate claim at the design
checkpoint; it was not a runtime reference defect. The owning link was later
repaired, and R12a/R12b root document checks passed. Keep the diagnostic as
provenance without treating it as a current blocker or fresh verification.

<a id="current-f1-implementation-audit-2026-10-10"></a>

## F1 source audit and subsequent scoped closure (2026-10-10)

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
  canonical cores and original test assertions remain. Committed locally as
  6b5adb0ae6ef40b890c4ff95247c762d13697a62 after both scoped commit reviews;
  all twenty-two blobs match the frozen manifest. Fresh F2 acceptance is separate.

Grant semantic inspection has found no additional confirmed authorization bug;
source review does not prove live revocation or conformance. Documentation also
needs current/historical reconciliation and precise ConfigData Cluster wording:
a matching Grant is unnecessary, while some readers still consult its store.
Fresh admission, dynamic traffic and full Standard conformance remain open.
