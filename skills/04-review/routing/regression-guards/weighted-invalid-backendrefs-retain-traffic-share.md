---
knowledge_type: regression-guards
---

# Weighted Invalid BackendRefs Retain Their Traffic Share

## Required failure share

Invalid backendRef slots retain their authored order and weight. Silently removing
invalid slots or redistributing their share to healthy siblings defeats the
weighted-invalid-reference contract. Target validation and Controller dependency
registration must not compact the backend vectors. Invalid references produce no
new target dependency or authorization/store read; that does not remove their
failure slots from the effective rule.

## Current classification and dispatch evidence

The canonical `refs::targets::is_standard_service` accepts only omitted/empty group
and omitted/exact `Service` kind. Literal `core`, ServiceImport, ServiceExternalName,
and other pseudo kinds classify as Invalid, retaining their weight. The presence
of an older `EdgionService::from_kind` arm does not establish that a native reference
can reach it. A standard Service object's actual type is a separate peer-resolution
concern; this page makes no reachability claim for those older pseudo-kind arms.

In the native HTTP compiler, `CompiledBackend::is_generic_selectable` in
`routes/http/match_unit.rs` includes compiled Service, EdgionBackend, and Invalid
variants, and `build_backend_selector` retains their original indices. Thus a valid
AI target is not excluded from that selector. The ordinary peer path in
`pg_upstream_peer.rs` rejects a selected EdgionBackend with
`UpstreamNotBackendRefs` and 503; AI dispatch has its separate plan and requires
permitted backend references. These are observed call-chain facts, not a new
authorization guarantee inferred from selector test counts.

Controller AI facts preserve one entry per authored/effective slot. Invalid or
Grant-denied targets become `Missing` without reading Service/EdgionBackend state;
valid denied references still retain recovery dependencies and RefNotPermitted
status. Target-only fact loans never grant permission themselves.

## Ordinary rules must reach weighted selection

F2-K8S-WEIGHT-01 found that compiler membership alone did not establish the
whole-request contract: shared AI admission rejected an ordinary rule containing
any Invalid forwarding class before its selector ran. Current-source Kubernetes
controls reproduced all-500 responses for valid plus literal-core targets, while
valid plus missing Service alternated 200/500.

`ai_rule_invalid_reason` applies AI admission only when an AiProxy is enabled or
an actual canonical EdgionBackend is declared. A rule with neither condition
leaves invalid targets to the native selector, even when resolved AI facts are
absent. This does not turn an invalid target into a Service or redistribute its
weight. An all-invalid rule still fails every selection; a zero-weight invalid
slot does not receive traffic.

Actual EdgionBackend declarations still require matching facts and retain variant,
provider, AiProxy-count/policy, promoted-slot and mixed-AI checks. An enabled
AiProxy on ordinary targets still fails admission. Invalid spellings that do not
classify as EdgionBackend remain invalid native slots, never an AI execution path.
Source regressions cover these boundaries; a Gateway publication regression checks
the captured gate and the retained valid/invalid backend slots. The shared
PartiallyInvalid integration suite covers same-name and different-name core
references, unsupported kind, all-invalid and zero-weight controls in both fixture
trees. Fresh correction verification passed 9 Resources admission tests and 41
Gateway publication tests. Current-binary Kubernetes PartiallyInvalid passed 4/4:
24 requests per mixed-invalid path yielded 12 HTTP 200 and 12 HTTP 500; the
all-invalid and zero-weight controls yielded 24 HTTP 500 and 24 HTTP 200,
respectively. The same-name/different-name core probe also recovered its 1:1
failure share. Evidence and preparation failures remain separately recorded under
`tmp/resource-reference-contract/k8s-weighted-fix-20261027/` and the reference
task's progress record. These bounded checks do not establish full source-mode
parity or a new Standard conformance result.

## Native HTTP error boundary

Malformed descriptors, including missing/out-of-range declared Service ports,
return `BackendUnsupportedKind`/500 before backend plugins, policy/SP lookups or
peer resolution. ReferenceGrant denial remains `RefDenied`/500. Missing valid
Services retain `BackendServiceNotFound`/500; unavailable live sections/endpoints
of valid Service targets retain their existing 503 errors. Invalid TLS policy
retains 500. A selected EdgionBackend that reaches ordinary peer dispatch returns
503; a real AI rule must first pass its separate admission gate. gRPC invalid/denied
slots use the existing HTTP 200/gRPC 14 wrapper.

The invalid-port 500 response deliberately supersedes the earlier late 503 failure.
Vendored Gateway API v1.6.2 HTTPBackendRef rules require invalid weighted shares
to return 500 and distinguish a valid Service without ready endpoints (503).
The historical `invalid_backend_with_service_key_still_captures` test name is
preserved, but its unsafe context-only policy-capture expectation is replaced by
None and a zero actual-lookup counter using the same malicious input and installed
policy. An Invalid slot can no longer recover Service authority from request context.

## L4 selected failure boundary

TCP, UDP and both retained TLS proxy implementations select original indices from
a private actual-Route generation. Invalid and denied slots keep their weights,
but the selected loan cannot call the discovery resolver. Denial wins over
malformation. Missing or out-of-range declared Service ports now fail terminally
as `UnsupportedBackend`; the former TLS `continue` transferred that failure share
to a sibling and is superseded. Valid live section/endpoint failure still permits
the existing TLS retry; connect retry limits/backoff remain unchanged. TCP/UDP
gain no retries. Existing UDP sessions remain pinned until removal or idle expiry.

## Historical evidence and re-evaluation

The `gw-12` resolution (2026-08-09) preserved weighted-invalid failure shares. Its
older description of core ServiceImport as a valid Service target, and valid AI
being excluded from the ordinary selector, is superseded by the current source
classification and selection evidence above. Historical WRR counts are not fresh
verification and do not override the reference contract.

Re-evaluate when changing canonical target classification, selector membership,
invalid-slot weighting, AI dispatch, or ordinary peer rejection. Trace current
source and tests before applying this page to a new finding.
