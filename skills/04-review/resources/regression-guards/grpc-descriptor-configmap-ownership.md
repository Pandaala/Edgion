---
name: grpc-descriptor-configmap-ownership
description: Use when reviewing GrpcJsonTranscoder ConfigMap acquisition, dependency and status ownership.
knowledge_type: regression-guards
---

# GrpcJsonTranscoder descriptor references retain actual ownership

## Conclusion and scope

EdgionPlugins GrpcJsonTranscoder configMapRef descriptors select only the fixed
core ConfigMap in their actual owner's namespace. An omitted owner namespace
defaults to default; an explicit malformed owner identity is rejected. The exact
binaryData key is part of the reference-only topology. Inline descriptor bytes
create no Kubernetes dependency and never enter that topology.

Only enabled supported request entries acquire material. Preparse and parse clear
previous descriptor material and errors in all four typed stages before binding.
Readonly status uses the same fixed target and permission pipeline without adding
dependencies. Identity errors report InvalidReference. Invalid key shape, missing
binaryData key, byte-cap and descriptor compilation errors retain the intrinsic
Accepted failure contract; a missing ConfigMap additionally reports ResolvedRefs.

## Rationale and evidence

The actual owning adapter is in Resources refs/plugin_fields.rs and owned.rs.
GrpcDescriptorReference has a private constructor and exposes its reference/key
only after identity and selector validation. Name, key, branch, enable and owner
drift invalidate a prepared loan; Controller material writeback does not.
Controller handlers/plugins/grpc_descriptor.rs consumes this loan through
credential/pipeline.rs. It never falls back from binaryData to data and cannot
choose another namespace, group or kind. Shared key-shape validation is the
unchanged crate-private grpc_transcoding::valid_descriptor_key predicate.

The applicable authority is the root AGENTS.md cross-cutting unification, serial
workqueue and schema/transport rules. This fixed local field has no authored
cross-namespace capability and requires no ReferenceGrant query for acquisition.

## Alternatives and limits

This is not a CA reference: converting ConfigMap contents to a Secret wrapper or
reading ca.crt would grant the wrong material semantics. Protobuf services[] are
not Kubernetes Service references. No new Gateway API field or CRD change follows.
HTTPRoute rule ExtensionRef remains the only supported runtime attachment.
The generic dependency-status scan was separate R4 work. R3a's scoped
ExtensionRef migration was one slice of the shared
[resource reference contract](../../../01-architecture/00-common/06-resource-reference-contract.md);
it does not establish descriptor-material or whole-task conformance acceptance.

## Verification and re-evaluation

Historical Controller-owning implementation evidence: scoped Astra review passed,
Resources tests passed 65, Resources doctests passed 16 with 10 ignored examples,
and Controller plugin handler tests passed 107 after sharing the key predicate.
These counts are historical, not the current Gateway verification.
Coverage includes malformed source/name/key with no dependency, default namespace,
all-stage clearing, exact binaryData selection, recovery and replacement/delete,
readonly status and actual Accepted/ResolvedRefs conditions.
Re-evaluate if descriptors gain namespace/group/kind fields, another storage branch,
another plugin stage or runtime attachment, or different missing-key status rules.
These results do not establish whole-task or live conformance completion.

## Resolution references

- `core-api-group-alias-contract` — closed 2026-11-12; resolved by the
  completed unified [resource reference contract](../../../01-architecture/00-common/06-resource-reference-contract.md),
  including its approved GrpcJsonTranscoder inventory addition.

## R3c Gateway consumption boundary (2026-10-08)

Gateway binds the actual equal-length, policy-masked EdgionPlugins request entries.
For ConfigMap descriptors, the same bound entry supplies config and selector;
validation precedes the config-compilation prepass, cache reuse and construction.
Malformed source/name/key cannot consume resolved descriptor bytes or reuse a
cached runtime. Shared name validation uses the canonical Kubernetes DNS predicate.
The detached factory accepts only inline descriptors, which have no object edge.
This validates ownership and target shape, not cryptographic material provenance.

Unsupported response-stage ConfigMap entries perform shared material-free intrinsic
limits/name/key checks, then existing alias/dye/stage checks. Intrinsic errors retain
priority; otherwise wrong-stage replaces descriptor-byte compilation diagnostics.
The Resources secondary body pass uses the same rule. Inline diagnostics, enabled
masks, entry ordinals, request/response bounds and transport ownership are unchanged.

Initial-candidate in-process evidence: Resources resource tests 1602 and transcoding tests 13;
Resources doctests 27 passed/10 ignored; Controller processor tests 929;
Gateway plugin tests 2260 passed/17 ignored. Raw commands, source/binary hashes and
identity comparisons were kept in local
`tmp/resource-reference-contract/r3c-verification-20261008-r1` evidence, which is
no longer retained.
The new worker test drives ConfigMap, EP, HTTPRoute and Gateway queues through exact
raw-store lookup and process_work_item, including changed bytes/status, deletion,
recreation, coalesced UID/generation replacement and dependency replacement/removal.
Namespaced effective Route targets match the current raw EP UID. Route conditions
remain independent of plugin Accepted; Gateway compilation owns fail-closed material
validation. Raw-None EP recovery and notification are proved; semantic target
lookup versus bare raw keys was left to R4 and resolved by R4d (`b872bc8c9`); no
alias is fabricated. Live conf_sync and
conformance are untested here. The initial candidate's first Astra result PASS and
14 clean gates (check 47.479 seconds, Clippy 80.239 seconds) are historical and
superseded by the second review's R3c-F1 BLOCK: upstream derived clones copied
resolved descriptor bytes before the owning guard. The R3c-F1 correction below
supersedes that candidate; the contract's final acceptance is recorded in its
[verification evidence](../../../01-architecture/00-common/06-resource-reference-contract.md#verification-evidence).

### R3c-F1 constructor-copy correction

One private cold-copy implementation binds the actual source before copying a
ConfigMap descriptor payload. Publication generation construction, policy-view
construction and direct compile all use it. Explicit spec/entry/config copies
preserve every other field and ordinal while invalid, inactive and unsupported
ConfigMap entries omit resolved bytes. The policy-view demand applies before its
copy; this does not retroactively prohibit an earlier eligible source generation
from owning valid material. Runtime rate-dependency validation now borrows original
aliases through the same canonical algorithm instead of cloning all entries.

The actual payload-copy callback is observed in deterministic tests: rejected and
undemanded payloads cause no copy invocation, while the eligible sibling copies
exact bytes. Full-field comparison, policy/cache/old-Arc tests and actual HTTP rule
publication delta/full/recreation tests cover the call sites. Source review remains
necessary to exclude an enclosing derived Clone bypass. Opaque raw input retention,
deserialization and source-equality serialization are input bookkeeping outside
executable construction; no process-wide zero-copy claim follows.

Fresh corrected verification (kept in local
`tmp/resource-reference-contract/r3c-f1-verification-20261008-r1` evidence, which
is no longer retained) passed Resources
1602 resource tests plus 13 transcoding tests, doctests 27/10 ignored, Controller
929 processor tests, and Gateway 2264 plugin tests/17 ignored. All 253 original
scoped IDs/assertions and the initial R3c regressions remain; source IDs total 267.
Complete compiled IDs are Resources 1931, Controller 1751 and Gateway 5158, with
no baseline removals. The corrected 19-path candidate then passed renewed
coordinator gates and two independent actual-result reviews and was committed as
`ca7bdaf79`; the earlier PASS alone did not accept it.
