---
name: gateway-api-extensionref-validation-fails-closed
description: Use when reviewing HTTPRoute or GRPCRoute ExtensionRef discriminator, target, status, delegation, or Gateway fail-closed behavior.
knowledge_type: regression-guards
---

# Gateway API ExtensionRef validation fails closed

## Conclusion and scope

HTTPRoute and GRPCRoute filters use one shared validation decision for the
`type`/`extensionRef` union and Edgion's supported target. `type: ExtensionRef`
requires an `extensionRef` payload, every other type forbids that payload, and
the supported plugin target is exactly `edgion.io/EdgionPlugins` with a valid
required Kubernetes resource name. An empty
group means the Kubernetes core API group; it is not an alias for `edgion.io`.

HTTP status and delegation distinguish an invalid name on the supported target
from an unsupported group/kind. After the canonical validator rejects the target,
the existing group/kind predicate selects the name-specific diagnostic
`ExtensionRef name must be a valid Kubernetes object name`. Unknown group/kind
keeps its existing unsupported-target wording even when the name is also invalid.
The shared validation result, filter-union priority and zero-plugin-edge behavior
are unchanged; gRPC retains its generic invalid-rule diagnostic.

The Controller applies this rule to rule and backendRef filters of every route,
including delegation children, which are validated as their own routes. Invalid
rules are dropped by exact index. A surviving sibling produces `PartiallyInvalid=True` with a message
starting with `Dropped Rule`; when no rule survives, the attachment is
`Accepted=False` and `PartiallyInvalid` is absent. The Gateway independently
seals malformed discriminator/payload unions and invalid filter-list construction
with the canonical generic 500 request gate. This existing gate does not establish
independent owner/name validation for ExtensionRef lookup or consumption. A
rule-scoped gate runs before CORS preflight, ordinary plugin execution, or
upstream processing; a backend-scoped gate runs only after that backend is
selected and before forwarding. Global and Gateway request-ID plugins may
already have run in Pingora's early hook before route matching identifies a
malformed route. A source-only AI diagnostic does not imply a dropped or
fallback rule and must not be represented by `PartiallyInvalid`.

## Owning-reference boundaries

R3a1 binds Controller plugin keys to actual HTTP/GRPC owners and rule/backend
filter positions. Invalid source/target identities create no new plugin edge or
lookup. Authored stamps are ignored; effective loans validate existing derived
locality. Original-child references register under the root wakeup before drops
and before desired-generation RawStore inspection. Parse entry replaces the old
set; later registration is additive, preserving valid missing/dropped targets
for repair, deletion and recreation. Scope checks retain their existing WAF,
RequestId, late-jump and gRPC-transcoder priority and disabled-entry semantics.
A standalone status call may withdraw stale dependencies and derived publication
state only when the source itself is invalid. Invalid targets, dropped rules,
missing plugins and scope errors under a valid owner retain additive status
registration. Zero-read guarantees here concern plugin reference discovery and
consumption, not unrelated status operations.

Gateway lookup admission now uses the same actual-owner field loans through
one cold key projection. HTTP/GRPC rule and backend builders receive actual
Routes and indexed fields. Invalid descriptors retain `None` in the rule key
vector and invoke no target lookup. Valid missing descriptors retain their keys
and fixed missing-reference error. Publication demand, AI/adaptive discovery,
CORS/WAF selection and access-log declarations cannot consume invalid same-name
references. Every field resolves in its declaring route's namespace; there is no
namespace stamp.

Whole-runtime malformed-union rejection remains separate from invalid-target
positioning. The latter keeps every original ExtensionRef stage filter and its
fixed error attribution. Preflight captures the first invalid rule filter's
existing Arc and dispatches a borrowed one-element slice through the ordinary
request runner. Leading policy/configuration gates retain precedence, and an
earlier CORS filter cannot terminate successfully first. Backend filters have no
rule preflight gate and execute only for the selected backend. The request's
captured class scope and existing backend cache preserve hits and misses through
stages and retries; body resolution creates no plugin log or key allocation.

## Rationale and evidence

Gateway API's HTTPRoute and GRPCRoute filter CEL declarations require the
discriminator and payload in both directions. `LocalObjectReference.group`
also defines an empty value as the core API group. The standard's
`PartiallyInvalid` condition is positive-only for implementations that drop
invalid rules, and its message prefix identifies that behavior.

`edgion_resources::resources::refs::extension_ref::validate_extension_ref_filter`
owns the shared decision. The HTTP and gRPC Controller handlers consume it for
source rules, delegation children included. The
Gateway HTTP and gRPC filter builders consume it before constructing any
filter chain. The matched rule runtime participates in the same preflight gate
as global, Gateway, and referenced plugin runtimes, preventing a rule-level
malformed filter from letting an OPTIONS request return 204 first. BackendRef
filters remain backend-scoped as Gateway API requires: they execute only after
that backend is selected for forwarding, so a locally terminated preflight does
not execute them. On the direct-Gateway defense path, this also prevents one
invalid weighted backend from poisoning valid siblings. Filesystem and etcd
input still passes through the Controller's admission-parity rule validation,
while Kubernetes normally rejects the malformed union through CEL. When plugin
policy adds an observational bypass note to a separately invalid runtime,
`RejectInvalidConfig` remains first; otherwise the preflight fast path would
mistake the note for an ordinary runtime and skip the gate.
Explicit unsupported targets retain the existing
`ExtensionRefFilter` attribution when malformed data bypasses the Controller;
only a missing or discriminator-mismatched payload uses the generic invalid
configuration gate.

## Alternatives and limits

Silently ignoring an absent payload is unsafe because a policy-bearing filter
disappears while traffic continues. Treating an empty group as `edgion.io`
would contradict Gateway API reference semantics. Rejecting an entire route
when a valid sibling remains would discard the standard's partial-validity
behavior.

Kubernetes admission normally rejects the malformed union first. The
Controller and Gateway checks remain necessary for filesystem, etcd, conf-sync,
and defense-in-depth paths. This guard does not change dangling
`EdgionPlugins` lookup behavior, backend-scoped WAF restrictions, response
streaming, or access-log ownership.

## Verification and re-evaluation

Focused unit coverage exercises the shared decision matrix; HTTP and gRPC
rule/backendRef status; delegation-child filtering; generic Gateway 500
behavior for rule and backend scopes; and policy-bypass ordering ahead of
preflight. Re-evaluate if Edgion supports another
ExtensionRef group/kind, Gateway API changes the union or partial-validity
contract, or the route ingestion architecture changes.

Historical verification on 2026-09-12 passed the focused resource, Controller,
delegation, publication, adaptive-routing, ExtensionRef runtime, and real
OPTIONS-session tests. `cargo check --workspace --all-targets`, `cargo clippy
--workspace --all-targets`, the agent-doc validator, SSA force guard, metrics
inventory guard, and Pingora source-policy guard also completed successfully.
Clippy reported only the repository's existing non-fatal warnings. Independent
final review returned LGTM after the preflight, policy-bypass ordering, backend
scope, and status corrections.

Fresh scoped verification on 2026-10-08 passed Resources reference tests (74),
Resources doctests (24 passed, 10 existing ignored), Controller resource-processor
tests (918), and readonly Gateway filter/publication/preflight/WAF selections
(53/23/23/65; filtered sets may overlap). Raw commands and exits are retained in
`tmp/resource-reference-contract/r3a1-verification-20261008-r1`. This verifies the
Controller migration and existing Gateway regressions; R3a2 lookup admission and
whole-task conformance remain open. Independent result review remains pending.

The F1 HTTP diagnostic correction subsequently passed 921 focused Controller
resource-processor tests, including name-specific rule/backend status and
delegated-child diagnostics with zero plugin dependencies. Correction logs and
complete identity comparisons are under
`tmp/resource-reference-contract/r3a1-verification-20261008-r2`; all original
compiled and source test identities are preserved. Both independent Astra result
reviewers accepted frozen r2 correctness with SCOPED PASS. Its Clippy command
exited zero with two introduced `question_mark` warnings in test helpers, so it
was not warning-free. A separately reviewed r3 cleanup replaces only those two
equivalent early returns. Fresh r3 verification passed the same 921 Controller
tests with preserved compiled/source identities and ignore attributes; command,
log and binary hashes are retained in
`tmp/resource-reference-contract/r3a1-verification-20261008-r3`. Refreshed lint
gates and independent byte review remain pending. This adds no Gateway
owner/name validation claim.

R3a1 subsequently committed locally as `05e7debf5`; its pending-gate statements
above describe earlier evidence checkpoints. R3a2 uses the reviewed 31-path
scope, including a supporting test-only AiProxy `api` fixture correction. Fresh
stable r4 verification passed the full Gateway runtime (424), publication (25),
adaptive (21), request-filter (29), resolver (nine), HTTP/GRPC match-unit (12/nine),
log-extension (15), context (eight), native-WAF (38), early-request (11), and
plugin-store (41) selections. Full compiled identities remain intact (5132 to
5144; 12 additions), and all 122 original scoped source tests, including the
four support tests, remain with unchanged ignore attributes. Raw failed attempts,
commands, exits and stable source/binary hashes are retained under
`tmp/resource-reference-contract/r3a2-verification-20261008-r1`. Workspace gates,
independent result reviews and the R3a2 commit remain coordinator-owned pending
steps. In-process missing/create/update/delete/recreate and policy-capture tests
are not live admission/conformance verification.

## Resolution references

- `fail-closed-missing-gateway-api-extensionref-configuration` — fixed 2026-09-12.

EdgionPlugins raw-source collisions are distinct from malformed ExtensionRef
syntax and unique-source execution-scope errors. Ambiguous effective targets
produce ResolvedRefs=False (InvalidPluginsRef), without changing Accepted,
PartiallyInvalid, effective rules or backend positions. The captured Gateway
scope contains a present sealed denial, including for global/Gateway consumers
that intentionally drop ordinary missing references. No candidate policy, AI
configuration or access-log declarations are selected; a fallback rule cannot
replace the ambiguous matched rule. Semantic dependency keys retain recovery
on either candidate's update or removal.
