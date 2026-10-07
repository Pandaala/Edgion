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
